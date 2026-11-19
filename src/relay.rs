
use std::{collections::HashMap, time::{Duration, SystemTime}};

use async_trait::async_trait;
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::{activation::ActivationInfo, util::{base64_decode, REQWEST}, DebugMeta, OSConfig, PushError, RegisterMeta};

#[derive(Deserialize)]
pub struct DataResp {
    data: String,
}

#[derive(Deserialize)]
pub struct VersionsResp {
    versions: Versions,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Versions {
    software_build_id: String,
    software_name: String,
    software_version: String,
    serial_number: String,
    hardware_version: String,
    unique_device_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ActivationResp {
    #[serde(rename = "Serial")]
    serial: ActivationIdentity,
}

// identity used for albert activation, provided by the relay's activate bridge command
// (hybrid mac identity - the relay owns this, do not reuse the iphone's version data here)
#[derive(Serialize, Deserialize, Clone)]
pub struct ActivationIdentity {
    pub product_type: String,
    pub activation_state: String,
    pub activation_randomness: String,
    pub device_class: String,
    pub unique_device_id: Option<String>,
    pub ua: Option<String>,
    pub build_version: String,
    pub product_version: String,
    pub activation_device: String,
    pub serial_number: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RelayConfig {
    pub version: Versions,
    pub icloud_ua: String,
    pub aoskit_version: String,
    pub dev_uuid: String,
    pub protocol_version: u32,
    pub host: String,
    pub code: String,
    pub beeper_token: Option<String>,
    pub udid: Option<String>,
    #[serde(default)]
    pub activation_identity: Option<ActivationIdentity>,
}

impl RelayConfig {
    pub async fn get_versions(host: &str, code: &str, beeper_token: &Option<String>) -> Result<Versions, PushError> {
        let mut data = REQWEST.post(format!("{}/api/v1/bridge/get-version-info", host))
            .bearer_auth(code)
            .header("Content-Length", "0");

        if let Some(token) = beeper_token {
            data = data.header("X-Beeper-Access-Token", token.clone());
        }

        let result = data.send().await?;

        match result.status().as_u16() {
            200 => {},
            404 => {
                return Err(PushError::DeviceNotFound)
            },
            _status => {
                return Err(PushError::RelayError(_status, result.text().await?))
            }
        }

        let result: VersionsResp = result.json().await?;

        Ok(result.versions)
    }

    pub async fn get_activation_identity(host: &str, code: &str, beeper_token: &Option<String>) -> Result<ActivationIdentity, PushError> {
        let mut data = REQWEST.post(format!("{}/api/v1/bridge/activate", host))
            .bearer_auth(code)
            .header("Content-Length", "0");

        if let Some(token) = beeper_token {
            data = data.header("X-Beeper-Access-Token", token.clone());
        }

        let result = data.send().await?;

        match result.status().as_u16() {
            200 => {},
            404 => {
                return Err(PushError::DeviceNotFound)
            },
            _status => {
                return Err(PushError::RelayError(_status, result.text().await?))
            }
        }

        let result: ActivationResp = result.json().await?;

        Ok(result.serial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_serial_identity() {
        let resp: ActivationResp = serde_json::from_str(r#"{"Serial":{"product_type":"iMac13,1","activation_state":"Unactivated","activation_randomness":"634657F8-6FA6-4A8A-B29D-AC033399B8C5","device_class":"MacOS","unique_device_id":null,"ua":null,"build_version":"22G513","product_version":"13.6.4","activation_device":"MacOS","serial_number":"C02WGSSWDNCR"}}"#).unwrap();
        assert_eq!(resp.serial.serial_number, "C02WGSSWDNCR");
        assert_eq!(resp.serial.product_type, "iMac13,1");
        assert_eq!(resp.serial.build_version, "22G513");
        assert!(resp.serial.unique_device_id.is_none());
        assert!(resp.serial.ua.is_none());
    }
}

#[async_trait]
impl OSConfig for RelayConfig {
    fn build_activation_info(&self, csr: Vec<u8>) -> ActivationInfo {
        // the relay supplies the mac-shaped activation identity (and a fresh randomness per call);
        // fall back to the old locally-built hybrid if it wasn't fetched
        let identity = self.activation_identity.clone().unwrap_or_else(|| ActivationIdentity {
            product_type: "iMac13,1".to_string(),
            activation_state: "Unactivated".to_string(),
            activation_randomness: Uuid::new_v4().to_string().to_uppercase(),
            device_class: "MacOS".to_string(),
            unique_device_id: None,
            ua: None,
            build_version: self.version.software_build_id.clone(),
            product_version: self.version.software_version.clone(),
            activation_device: "MacOS".to_string(),
            serial_number: self.version.serial_number.clone(),
        });

        ActivationInfo {
            activation_randomness: identity.activation_randomness,
            activation_state: match identity.activation_state.as_str() {
                "Activated" => "Activated",
                _ => "Unactivated",
            },
            build_version: identity.build_version,
            device_cert_request: csr.into(),
            device_class: identity.device_class,
            product_type: identity.product_type,
            product_version: identity.product_version,
            serial_number: identity.serial_number,
            unique_device_id: identity.unique_device_id.unwrap_or_else(|| self.version.unique_device_id.clone()).to_uppercase(),
        }
    }

    fn get_udid(&self) -> String {
        // all icloud service requests claim the relay's iphone identity, so the udid
        // must be the iphone's too (the activation mac identity has no udid of its own).
        // fall back to the version identity's udid if no explicit override is set
        self.udid.clone().unwrap_or_else(|| self.version.unique_device_id.clone())
    }

    fn get_normal_ua(&self, item: &str) -> String {
        let part = self.icloud_ua.split_once(char::is_whitespace).unwrap().0;
        format!("{item} {part}")
    }

    fn get_serial_number(&self) -> String {
        self.version.serial_number.clone()
    }

    fn get_mme_clientinfo(&self, item: &str) -> String {
        format!("<{}> <{};{};{}> <{}>", self.version.hardware_version, self.version.software_name, self.version.software_version, self.version.software_build_id, item)
    }

    fn get_adi_mme_info(&self, item: &str, require_mac: bool) -> String {
        if require_mac {
            // must be mac for ClearADI
            format!("<iMac13,1> <macOS;13.6.4;22G513> <{}>", item)
        } else {
            // oct 2026 grand slam break: apple rejects iphone os claims with -80045.
            // use iloader/isideload's verified working claim (see iloader src-tauri/src/account.rs,
            // working as of late oct 2026, "temporary while waiting for the anisette servers
            // to start reporting this")
            format!("<Mac15,7> <macOS;27.0;26A5378j> <{}>", item)
        }
    }

    fn get_aoskit_version(&self) -> String {
        self.aoskit_version.clone()
    }

    fn get_gsa_hardware_headers(&self) -> HashMap<String, String> {
        // oct 2026 grand slam break: a real device serial makes apple resolve the platform
        // (relay's iphone serial -> iphone -> -80045 even with a mac claim). verified working
        // implementations (iload/isideload) send no serial at all, so don't send one either
        HashMap::new()
    }

    fn get_version_ua(&self) -> String {
        format!("[{},{},{},{}]", self.version.software_name, self.version.software_version, self.version.software_build_id, self.version.hardware_version)
    }

    fn get_login_url(&self) -> &'static str {
        "https://setup.icloud.com/setup/signin/v2/login"
    }

    fn get_activation_device(&self) -> String {
        self.activation_identity.as_ref().map(|identity| identity.activation_device.clone()).unwrap_or_else(|| "MacOS".to_string())
    }

    fn get_device_uuid(&self) -> String {
        self.dev_uuid.clone()
    }

    fn get_device_name(&self) -> String {
        format!("iPhone-{}", self.version.serial_number)
    }

    fn get_protocol_version(&self) -> u32 {
        self.protocol_version
    }

    async fn generate_validation_data(&self) -> Result<Vec<u8>, PushError> {
        let mut data = REQWEST.post(format!("{}/api/v1/bridge/get-validation-data", self.host))
            .bearer_auth(&self.code)
            .header("Content-Length", "0");

        if let Some(token) = &self.beeper_token {
            data = data.header("X-Beeper-Access-Token", token.clone());
        }

        let result = data.send().await?;

        match result.status().as_u16() {
            200 => {},
            404 => {
                return Err(PushError::DeviceNotFound)
            },
            _status => {
                return Err(PushError::RelayError(_status, result.text().await?))
            }
        }

        let result: DataResp = result.json().await?;

        Ok(base64_decode(&result.data))
    }

    fn get_register_meta(&self) -> RegisterMeta {
        RegisterMeta {
            hardware_version: self.version.hardware_version.clone(),
            os_version: format!("{},{},{}", self.version.software_name, self.version.software_version, self.version.software_build_id),
            software_version: self.version.software_build_id.clone(),
        }
    }

    fn get_debug_meta(&self) -> DebugMeta {
        DebugMeta {
            user_version: self.version.software_version.clone(),
            hardware_version: self.version.hardware_version.clone(),
            serial_number: self.version.serial_number.clone(),
        }
    }

    fn get_private_data(&self) -> Dictionary {
        let apple_epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(978307200);
        Dictionary::from_iter([
            // apple pay
            ("ap", Value::String("0".to_string())),

            ("d", Value::String(format!("{:.6}", apple_epoch.elapsed().unwrap().as_secs_f64()))),
            // device type
            ("dt", Value::Integer(1.into())),
            // green tea - ??
            ("gt", Value::String("0".to_string())),
            // supports handoff
            ("h", Value::String("1".to_string())),
            // supports phone calls
            ("p", Value::String("0".to_string())),

            ("pb", Value::String(self.version.software_build_id.clone())),
            ("pn", Value::String(if self.version.software_name == "MacOS" { "macOS".to_string() } else { self.version.software_name.clone() })),
            ("pv", Value::String(self.version.software_version.clone())),
            
            // mms router support
            ("m", Value::String("1".to_string())),
            // sms router support
            ("s", Value::String("1".to_string())),

            // tethering support
            // ec = enclosure color
            // c = data color
            // ss = service signatures
            // ktf = key transparency flags
            // ktv = key transparency version
            ("t", Value::String("0".to_string())),
            ("u", Value::String(self.dev_uuid.clone().to_uppercase())),
            // version
            ("v", Value::String("1".to_string())),
        ])
    }

}
