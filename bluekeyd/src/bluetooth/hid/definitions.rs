use bluer::Uuid;

pub mod characteristics {
    use bluer::Uuid;

    // UUIDs for each of the different characteristics used by an HID device
    pub const PROTOCOL_MODE: Uuid = Uuid::from_u128(0x00002A4E_0000_1000_8000_00805F9B34FB);
    pub const REPORT: Uuid = Uuid::from_u128(0x00002A4D_0000_1000_8000_00805F9B34FB);
    pub const REPORT_MAP: Uuid = Uuid::from_u128(0x00002A4B_0000_1000_8000_00805F9B34FB);
    pub const INFORMATION: Uuid = Uuid::from_u128(0x00002A4A_0000_1000_8000_00805F9B34FB);
    pub const CONTROL_POINT: Uuid = Uuid::from_u128(0x00002A4C_0000_1000_8000_00805F9B34FB);
    pub mod boot {
        use bluer::Uuid;
        #[allow(dead_code)]
        pub const MOUSE_OUTPUT: Uuid = Uuid::from_u128(0x00002A33_0000_1000_8000_00805F9B34FB);

        pub mod keyboard {
            use bluer::Uuid;

            pub const OUTPUT: Uuid = Uuid::from_u128(0x00002A32_0000_1000_8000_00805F9B34FB);
            pub const INPUT: Uuid = Uuid::from_u128(0x00002A22_0000_1000_8000_00805F9B34FB);
        }
    }

    
}

// UUID for the HID service
pub const SERVICE: Uuid = Uuid::from_u128(0x00001812_0000_1000_8000_00805F9B34FB);

pub mod descriptors {
    use bluer::Uuid;

    // UUID for a report type descriptor
    pub const REPORT_REFERENCE: Uuid = Uuid::from_u128(0x00002908_0000_1000_8000_00805F9B34FB);
}




