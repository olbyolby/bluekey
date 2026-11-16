
// All HID characteristics require encryption(security mode 1 level 2, as HOGP requires). The host has to pair
// before using the service, so it subscribes to input reports under its permanent(identity) address, which BlueZ
// keeps the subscription for across reconnections, instead of a temporary address that is forgotten on disconnect.
use bluer::gatt::local::{Characteristic, CharacteristicNotify, CharacteristicNotifyFun, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicReadFun, CharacteristicWrite, CharacteristicWriteFun, CharacteristicWriteMethod, Descriptor, DescriptorRead};
use log::debug;
use super::definitions;


// The amount of cloning nonsense I had to do here is going to drive me to extremism
// Blur's callbacks want some weird cursed call signature and that gets repetitive to write.
// First a clone needs to be made so it can be moved into the callback without consuming the state for everyone,
// Then another copy as to be made into the async section of the callback because the async bit may outlive the function.
#[macro_export]
macro_rules! callback {
    (|$($arg:ident),*| $($state:ident),+ $code:block) => {
        {
            #[allow(unused_parens)] // Silence compiler lints about this
            let ($($state),*) = ($($state.clone()),*);
            Box::new(move |$($arg),*| {
                #[allow(unused_parens)] 
                let ($($state),*) = ($($state.clone()),*);
                Box::pin(async move $code)
            })
        }
    };
    (|$($arg:ident),*| $code:block) => {
        Box::new(move |$($arg),*| {
            Box::pin(async move $code)
        })
    }
}
pub use crate::callback;

// Required for any HID device implementing the boot protocol, allows host to set which protocol to use
// Keyboard or mice should default to the boot protocal is supported
pub fn protocol_mode(read: CharacteristicReadFun, write: CharacteristicWriteFun) -> Characteristic {

    Characteristic {
        uuid: definitions::characteristics::PROTOCOL_MODE,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: read,
            ..Default::default()
        }),
        write: Some(CharacteristicWrite {
            encrypt_write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(write),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Required HID characteristic with some basic information about the device(localization and a few flags)
pub fn information(descriptor: &'static [u8]) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::INFORMATION,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: Box::new(move |_request| Box::pin(async move {
                Ok(descriptor.into())
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Required HID characteristic, 0x01 written when host sleeps, 0x00 on wake
pub fn control_point(write: CharacteristicWriteFun) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::CONTROL_POINT,
        write: Some(CharacteristicWrite { 
            encrypt_write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(write),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Required HID characteristic for supplying the HID Report descriptor to the host
pub fn report_map(read: CharacteristicReadFun) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::REPORT_MAP,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: read,
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Allows a keyboard to support the boot protocol, used for simpler devices/OSes(like BIOSes or UEFIs)
pub fn boot_keyboard_input(reader: CharacteristicReadFun, notify: CharacteristicNotifyFun) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::boot::keyboard::INPUT,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: reader,
            ..Default::default()
        }),
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(notify),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Same as above but for writing LED states
pub fn boot_keyboard_output(reader: CharacteristicReadFun, writer: CharacteristicWriteFun) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::boot::keyboard::OUTPUT,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: reader,
            ..Default::default()
        }),
        write: Some(CharacteristicWrite {
            encrypt_write: true,
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(writer),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Allows a mouse to work in boot mode, for things like UEFIs
pub fn boot_mouse_input(reader: CharacteristicReadFun, notify: CharacteristicNotifyFun) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::boot::MOUSE_OUTPUT,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: reader,
            ..Default::default()
        }),
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(notify),
            ..Default::default()
        }),
        ..Default::default()
    }
}
// Required HID characteristic for sending HID reports to the host
// `report_id` must match the Report ID used for this report in the Report Map (0 = no Report IDs)
pub fn input_report(read: CharacteristicReadFun, notify: CharacteristicNotifyFun, report_id: u8) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::REPORT,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: read,
            ..Default::default()
        }),
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(notify),
            ..Default::default()
        }),
        descriptors: vec![Descriptor {
            uuid: definitions::descriptors::REPORT_REFERENCE,
            read: Some(DescriptorRead {
                read: true,
                fun: Box::new(move |request| {
                    Box::pin(async move {
                        debug!("Descirptor for HID_REPORT INPUT (id {}) read by {}", report_id, request.device_address);
                        Ok([report_id, 0x01].into())
                    })
                }),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}
// HID Charactersitc allowing the host to send updates to the device(like LED updates)
// `report_id` must match the Report ID used for this report in the Report Map (0 = no Report IDs)
pub fn output_report(read: CharacteristicReadFun, write: CharacteristicWriteFun, report_id: u8) -> Characteristic {
    Characteristic {
        uuid: definitions::characteristics::REPORT,
        read: Some(CharacteristicRead {
            read: true,
            encrypt_read: true,
            fun: read,
            ..Default::default()
        }),
        write: Some(CharacteristicWrite {
            encrypt_write: true,
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(write),
            ..Default::default()
        }),
        descriptors: vec![Descriptor {
            uuid: definitions::descriptors::REPORT_REFERENCE,
            read: Some(DescriptorRead {
                read: true,
                fun: Box::new(move |request| {
                    Box::pin(async move {
                        debug!("Descirptor for HID_REPORT OUTPUT (id {}) read by {}", report_id, request.device_address);
                        Ok([report_id, 0x02].into())
                    })
                }),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}