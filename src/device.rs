use hidapi::{HidApi, HidDevice};
use std::fmt;

const VID: u16 = 0x16D0;
const PID: u16 = 0x0AAA;

pub struct FdsStick {
    dev: HidDevice,
}

#[derive(Debug)]
pub enum DeviceError {
    NotFound,
    Hid(hidapi::HidError),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::NotFound => write!(f, "FDS Stick not found (VID={VID:#06x} PID={PID:#06x})"),
            DeviceError::Hid(e) => write!(f, "HID error: {e}"),
        }
    }
}

impl From<hidapi::HidError> for DeviceError {
    fn from(e: hidapi::HidError) -> Self {
        DeviceError::Hid(e)
    }
}

impl FdsStick {
    pub fn open() -> Result<Self, DeviceError> {
        let api = HidApi::new()?;
        let dev = api.open(VID, PID).map_err(|_| DeviceError::NotFound)?;
        Ok(FdsStick { dev })
    }

    /// Send a HID SET_REPORT (feature report) to the device.
    /// `data` must start with the report ID byte.
    pub fn set_report(&self, data: &[u8]) -> Result<(), DeviceError> {
        self.dev.send_feature_report(data)?;
        Ok(())
    }

    /// Send a HID output report (via interrupt endpoint).
    /// `data` must start with the report ID byte.
    pub fn write_output(&self, data: &[u8]) -> Result<(), DeviceError> {
        self.dev.write(data)?;
        Ok(())
    }

    /// Send a HID GET_REPORT (feature report) from the device.
    /// `buf` must have buf[0] set to the report ID. The device fills the rest.
    /// Returns the number of bytes read.
    pub fn get_report(&self, buf: &mut [u8]) -> Result<usize, DeviceError> {
        let n = self.dev.get_feature_report(buf)?;
        Ok(n)
    }
}
