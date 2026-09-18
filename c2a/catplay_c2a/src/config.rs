use std::path::PathBuf;

use serde::Deserialize;

/// Platform kinds with verified bring-up. Adding a value here claims that the
/// platform's USB role switching, MFi bus and adapter naming were checked on
/// real hardware.
const SUPPORTED_PLATFORM_KINDS: &[&str] = &["generic", "rpi4", "rock2a"];

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub debug: bool,
    /// Legacy spelling retained for one migration release.
    pub persist_dir: Option<String>,

    pub platform: PlatformConfig,
    pub usb: UsbConfig,
    pub mfi: MfiConfig,
    pub bluetooth: BluetoothConfig,
    pub wifi: WifiConfig,

    /// Legacy network and gadget sections retained for existing firmware.
    pub wifi_network: WifiNetwork,
    pub gadget: GadgetConfig,
}

impl AppConfig {
    pub fn state_dir(&self) -> PathBuf {
        self.platform
            .state_dir
            .as_ref()
            .or(self.persist_dir.as_ref())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/catplay"))
    }

    pub fn resolved_usb_car(&self) -> ResolvedUsbCarConfig {
        match self.usb.car.as_ref() {
            Some(car) => ResolvedUsbCarConfig {
                enabled: car.enabled,
                udc: car.udc.clone(),
                power_mode: car.power_mode,
                pinned: car.pinned,
            },
            None => ResolvedUsbCarConfig {
                enabled: self.gadget.enabled,
                udc: self.gadget.udc_car.clone(),
                power_mode: UsbPowerMode::CarVbusSingleCable,
                pinned: self.gadget.pinned,
            },
        }
    }

    pub fn resolved_wifi(&self) -> ResolvedWifiConfig {
        ResolvedWifiConfig {
            enabled: self.wifi.enabled,
            device: self.wifi.device.clone(),
            country: self.wifi.country.clone(),
            ssid: self.wifi.ssid.clone().unwrap_or_else(|| self.wifi_network.ssid.clone()),
            password: self.wifi.password.clone().unwrap_or_else(|| self.wifi_network.password.clone()),
            wpa: self.wifi.wpa.unwrap_or(self.wifi_network.wpa),
            channel: self.wifi.channel.or(self.wifi_network.channel),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        let usb = self.resolved_usb_car();
        let wifi = self.resolved_wifi();

        if !SUPPORTED_PLATFORM_KINDS.contains(&self.platform.kind.as_str()) {
            errors.push(format!(
                "unsupported platform.kind {:?} (supported: {})",
                self.platform.kind,
                SUPPORTED_PLATFORM_KINDS.join(", ")
            ));
        }
        if self.state_dir().as_os_str().is_empty() {
            errors.push("platform.state_dir must not be empty".into());
        }
        if usb.enabled && usb.udc.as_deref().is_some_and(str::is_empty) {
            errors.push("usb.car.udc must not be empty".into());
        }
        if self.mfi.client.is_none() && self.mfi.i2c.is_none() {
            errors.push("mfi backend not defined".into());
        }
        if let Some(i2c) = self.mfi.i2c.as_ref() {
            if let Err(err) = i2c.device_path() {
                errors.push(err);
            }
            match i2c.address() {
                Ok(0x10 | 0x11) => {}
                Ok(address) => errors.push(format!("mfi.i2c.address must be the verified 0x10 or 0x11, got 0x{address:02x}")),
                Err(err) => errors.push(err),
            }
            if i2c.timeout_ms == 0 {
                errors.push("mfi.i2c.timeout_ms must be greater than zero".into());
            }
            if i2c.retry_delay_ms == 0 {
                errors.push("mfi.i2c.retry_delay_ms must be greater than zero".into());
            }
        }
        if self.bluetooth.enabled && self.bluetooth.device.is_empty() {
            errors.push("bluetooth.device must not be empty".into());
        }
        if wifi.enabled {
            if wifi.device.is_empty() {
                errors.push("wifi.device must not be empty".into());
            }
            if wifi.country.len() != 2 || !wifi.country.bytes().all(|b| b.is_ascii_uppercase()) {
                errors.push("wifi.country must be an uppercase ISO-3166 alpha-2 code".into());
            }
            if wifi.ssid.is_empty() || wifi.ssid.len() > 32 {
                errors.push("wifi.ssid must contain 1..32 bytes".into());
            }
            if wifi.wpa && !(8..=63).contains(&wifi.password.len()) {
                errors.push("wifi.password must contain 8..63 bytes when WPA is enabled".into());
            }
            if wifi.channel.is_none() {
                errors.push("wifi.channel must be explicit so iAP2 and the AP cannot diverge".into());
            }
        }
        // The wireless gadget needs the AP *and* the iAP2 Bluetooth profile, so
        // the two must be enabled together. Enabling neither is allowed: that
        // is the wired-only bring-up state used while validating a new board's
        // USB and MFi path before the wireless stack is proven.
        if usb.enabled && self.bluetooth.enabled != wifi.enabled {
            errors.push("bluetooth.enabled and wifi.enabled must be set together: the wireless gadget needs both".into());
        }

        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct PlatformConfig {
    pub kind: String,
    pub state_dir: Option<String>,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            kind: "generic".into(),
            state_dir: None,
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct UsbConfig {
    pub car: Option<UsbCarConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct UsbCarConfig {
    pub enabled: bool,
    pub udc: Option<String>,
    pub power_mode: UsbPowerMode,
    pub pinned: bool,
}

impl Default for UsbCarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            udc: None,
            power_mode: UsbPowerMode::CarVbusSingleCable,
            pinned: false,
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UsbPowerMode {
    #[default]
    CarVbusSingleCable,
    ManagedVbus,
}

#[derive(Debug, Clone)]
pub struct ResolvedUsbCarConfig {
    pub enabled: bool,
    pub udc: Option<String>,
    pub power_mode: UsbPowerMode,
    pub pinned: bool,
}

// MFi

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct MfiConfig {
    pub server: Option<MfiConfigServer>,
    pub client: Option<MfiConfigClient>,
    pub i2c: Option<MfiConfigI2C>,
    pub selftest: bool,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct MfiConfigServer {
    pub enabled: bool,
    pub bind: String,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct MfiConfigClient {
    pub remote: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct MfiConfigI2C {
    pub device: Option<String>,
    pub address: Option<u8>,

    /// Legacy fields. Prefer an explicit stable device path and address.
    pub bus_offset: Option<u32>,
    pub dev_addr: Option<u8>,

    pub timeout_ms: u64,
    pub retry_delay_ms: u64,
}

impl Default for MfiConfigI2C {
    fn default() -> Self {
        Self {
            device: None,
            address: None,
            bus_offset: None,
            dev_addr: None,
            timeout_ms: 2_000,
            retry_delay_ms: 5,
        }
    }
}

impl MfiConfigI2C {
    pub fn device_path(&self) -> Result<String, String> {
        if let Some(path) = self.device.as_ref() {
            if path.starts_with("/dev/i2c-") {
                return Ok(path.clone());
            }
            return Err(format!("mfi.i2c.device must be an absolute /dev/i2c-* path, got {path:?}"));
        }
        self.bus_offset
            .map(|bus| format!("/dev/i2c-{bus}"))
            .ok_or_else(|| "mfi.i2c.device (or legacy bus_offset) is required".into())
    }

    pub fn address(&self) -> Result<u8, String> {
        self.address
            .or(self.dev_addr)
            .ok_or_else(|| "mfi.i2c.address (or legacy dev_addr) is required".into())
    }
}

// Bluetooth

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct BluetoothConfig {
    pub enabled: bool,
    pub device: String,
    pub name: String,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device: "hci0".into(),
            name: "CatPlay".into(),
        }
    }
}

// Wi-Fi

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct WifiConfig {
    pub enabled: bool,
    pub device: String,
    pub country: String,
    pub ssid: Option<String>,
    pub password: Option<String>,
    pub wpa: Option<bool>,
    pub channel: Option<u8>,
}

impl Default for WifiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device: "wlan0".into(),
            country: "CN".into(),
            ssid: None,
            password: None,
            wpa: None,
            channel: None,
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct WifiNetwork {
    pub ssid: String,
    pub password: String,
    pub wpa: bool,
    pub channel: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct ResolvedWifiConfig {
    pub enabled: bool,
    pub device: String,
    pub country: String,
    pub ssid: String,
    pub password: String,
    pub wpa: bool,
    pub channel: Option<u8>,
}

// Legacy gadget section

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct GadgetConfig {
    pub enabled: bool,
    pub pinned: bool,
    pub udc_car: Option<String>,
    pub udc_extra: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rpi4_config_and_resolves_new_fields() {
        let config: AppConfig = toml::from_str(
            r#"
            [platform]
            kind = "rpi4"
            state_dir = "/var/lib/catplay"
            [usb.car]
            udc = "fe980000.usb"
            power_mode = "car-vbus-single-cable"
            [mfi]
            selftest = true
            [mfi.i2c]
            device = "/dev/i2c-1"
            address = 0x10
            [wifi]
            enabled = true
            device = "wlan0"
            country = "CN"
            ssid = "CatPlay"
            password = "12345678"
            wpa = true
            channel = 36
            [bluetooth]
            enabled = true
            device = "hci0"
            name = "CatPlay"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
        assert_eq!(config.mfi.i2c.as_ref().unwrap().device_path().unwrap(), "/dev/i2c-1");
        assert_eq!(config.resolved_usb_car().udc.as_deref(), Some("fe980000.usb"));
        assert_eq!(config.resolved_wifi().channel, Some(36));
    }

    #[test]
    fn parses_rock2a_config_and_resolves_new_fields() {
        let config: AppConfig = toml::from_str(
            r#"
            [platform]
            kind = "rock2a"
            state_dir = "/var/lib/catplay"
            [usb.car]
            udc = "fe500000.dwc3"
            power_mode = "car-vbus-single-cable"
            [mfi]
            selftest = true
            [mfi.i2c]
            device = "/dev/i2c-0"
            address = 0x10
            timeout_ms = 2000
            retry_delay_ms = 5
            [wifi]
            enabled = true
            device = "wlan0"
            country = "CN"
            ssid = "CatPlay"
            password = "12345678"
            wpa = true
            channel = 36
            [bluetooth]
            enabled = true
            device = "hci1"
            name = "CatPlay"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
        assert_eq!(config.mfi.i2c.as_ref().unwrap().device_path().unwrap(), "/dev/i2c-0");
        assert_eq!(config.resolved_usb_car().udc.as_deref(), Some("fe500000.dwc3"));
        assert_eq!(config.resolved_wifi().channel, Some(36));
    }

    #[test]
    fn rejects_unknown_platform_kind() {
        let config: AppConfig = toml::from_str(
            r#"
            [platform]
            kind = "not-a-board"
            [mfi.i2c]
            device = "/dev/i2c-0"
            address = 0x10
            "#,
        )
        .unwrap();

        let error = config.validate().unwrap_err();
        assert!(error.contains("unsupported platform.kind"), "{error}");
        assert!(error.contains("rock2a"), "{error}");
    }

    #[test]
    fn legacy_i2c_and_network_fields_still_resolve() {
        let config: AppConfig = toml::from_str(
            r#"
            persist_dir = "/tmp/catplay"
            [mfi]
            [mfi.i2c]
            bus_offset = 1
            dev_addr = 16
            [wifi]
            enabled = true
            device = "wlan0"
            [wifi_network]
            ssid = "Legacy"
            password = "12345678"
            wpa = true
            channel = 36
            [bluetooth]
            enabled = true
            device = "hci0"
            name = "Legacy"
            [gadget]
            enabled = true
            udc_car = "dummy_udc.0"
            "#,
        )
        .unwrap();

        config.validate().unwrap();
        assert_eq!(config.mfi.i2c.as_ref().unwrap().device_path().unwrap(), "/dev/i2c-1");
        assert_eq!(config.resolved_wifi().ssid, "Legacy");
    }
}
