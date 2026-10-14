#[cfg(feature = "nusb")]
use std::borrow::Cow;

#[cfg(feature = "nusb")]
pub(crate) fn nusb_speed_to_number(speed: nusb::Speed) -> u64 {
    match speed {
        nusb::Speed::Low => 1_500_000,
        nusb::Speed::Full => 12_000_000,
        nusb::Speed::High => 480_000_000,
        nusb::Speed::Super => 5_000_000_000,
        nusb::Speed::SuperPlus => 10_000_000_000,
        unknown => {
            tracing::warn!("unknown device speed: {unknown:?}");
            0
        }
    }
}

#[cfg(feature = "rusb")]
pub(crate) fn rusb_speed_to_number(speed: rusb::Speed) -> u64 {
    match speed {
        rusb::Speed::Low => 1_500_000,
        rusb::Speed::Full => 12_000_000,
        rusb::Speed::High => 480_000_000,
        rusb::Speed::Super => 5_000_000_000,
        rusb::Speed::SuperPlus => 10_000_000_000,
        unknown => {
            tracing::warn!("unknown device speed: {unknown:?}");
            0
        }
    }
}

#[cfg(feature = "nusb")]
pub(crate) fn to_udid(serial_number: &str) -> Cow<'_, str> {
    if serial_number.len() == 24 {
        let mut new_serial_num = String::with_capacity(25);
        new_serial_num.push_str(&serial_number[..8]);
        new_serial_num.push('-');
        new_serial_num.push_str(&serial_number[8..]);

        Cow::Owned(new_serial_num)
    } else {
        Cow::Borrowed(serial_number)
    }
}

#[cfg(feature = "rusb")]
pub(crate) fn to_udid_owned(serial_number: String) -> String {
    if serial_number.len() == 24 {
        let mut new_serial_num = String::with_capacity(25);
        new_serial_num.push_str(&serial_number[..8]);
        new_serial_num.push('-');
        new_serial_num.push_str(&serial_number[8..]);

        new_serial_num
    } else {
        serial_number
    }
}

pub(crate) fn is_disconnect_io(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}
