pub(super) mod characteristics;
pub(super) mod definitions;

pub(super) const HID_INFORMATION: &'static [u8] = &[
    0x01, 0x11, // HID spec
    0x00, // Country code
    0b00000010 //flags
];

#[derive(Clone, Copy, Debug)]
pub(super) enum Protocol {
    Boot,
    Report
}
impl Into<u8> for Protocol {
    fn into(self) -> u8 {
        match self {
            Self::Boot => 0,
            Self::Report => 1
        }
    }
}
