// A very awful, but technically functional, Bluetooth keyboard emulator(and an unused battery emulator)
use bluer::{Uuid, adv::Advertisement, gatt::{CharacteristicWriter, WriteOp::Request, local::{Application, Characteristic, CharacteristicControlEvent, CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Descriptor, DescriptorRead, Service, characteristic_control}}};

use futures::{FutureExt, StreamExt, pin_mut};
use std::{sync::{Arc}, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::sleep,
    sync::RwLock
};



const HID_SERVICE: Uuid = Uuid::from_u128(0x00001812_0000_1000_8000_00805F9B34FB);
const HID_REPORT_MAP: Uuid = Uuid::from_u128(0x00002A4B_0000_1000_8000_00805F9B34FB);
const HID_INFORMATION: Uuid = Uuid::from_u128(0x00002A4A_0000_1000_8000_00805F9B34FB);
const HID_CONTROL_POINT: Uuid = Uuid::from_u128(0x00002A4C_0000_1000_8000_00805F9B34FB);
const HID_BOOT_KEYBOARD_OUTPUT: Uuid = Uuid::from_u128(0x00002A32_0000_1000_8000_00805F9B34FB);
const HID_BOOT_KEYBOARD_INPUT: Uuid = Uuid::from_u128(0x00002A22_0000_1000_8000_00805F9B34FB);
const HID_PROTOCOL: Uuid = Uuid::from_u128(0x00002A4E_0000_1000_8000_00805F9B34FB);
const HID_REPORT: Uuid = Uuid::from_u128(0x00002A4D_0000_1000_8000_00805F9B34FB);
const REPORT_REFERENCE: Uuid = Uuid::from_u128(0x00002908_0000_1000_8000_00805F9B34FB);

const REPORT_DESCRIPTOR: &'static [u8] = &[
    0x05,0x01,0x09,0x06,0xA1,0x01,0x05,0x07,0x19,0xE0,0x29,0xE7,0x15,0x00,0x25,0x01,0x75,0x01,0x95,0x08,0x81,0x02,0x95,0x01,0x75,0x08,0x81,0x01,0x95,0x05,0x75,0x01,0x05,0x08,0x19,0x01,0x29,0x05,0x91,0x02,0x95,0x01,0x75,0x03,0x91,0x01,0x95,0x06,0x75,0x08,0x15,0x00,0x25,0x65,0x05,0x07,0x19,0x00,0x29,0x65,0x81,0x00,0xC0
];
const HID_INFORMATION_BIN: &'static [u8] = &[
    0x01, 0x11, // HID spec
    0x00, // Country code
    0b00000100 //flags
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    env_logger::init();
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Advertising on Bluetooth adapter {} with address {}", adapter.name(), adapter.address().await?);
    let advertisement = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: [HID_SERVICE].into(),
        appearance: Some(0x03C1),
        discoverable: Some(true),
        local_name: Some("HID test".to_string()),
        ..Default::default()
    };
    println!("{:?}", &advertisement);
    let handle = adapter.advertise(advertisement).await?;

    println!("Serving GATT HID service on Bluetooth adapter {}", adapter.name());
    let (hid_control_point_control, hid_control_point_handle) = characteristic_control();
    let (boot_keyboard_input_control, boot_keyboard_input_handle) = characteristic_control();
    let (input_report_control, input_report_handle) = characteristic_control();
    let app = Application {
        services: vec![Service {
            uuid: HID_SERVICE,
            primary: true,
            characteristics: vec![Characteristic {
                uuid: HID_PROTOCOL,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|request| Box::pin(async move {
                        println!("HID_PROTOCOL read by {} from {}", request.adapter_name, request.device_address);
                        Ok(vec![0])
                    })),
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Fun(Box::new(|_value, _req| Box::pin(async move {
                        println!("Ignoring write of {:?} to protocol by {} at {:?}", _value, _req.adapter_name, _req.device_address);
                        Ok(())
                    }))),
                    ..Default::default()
                }),
                ..Default::default()
            }, Characteristic {
                uuid: HID_INFORMATION,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|request| Box::pin(async move {
                        println!("HID_INFORMATION read by {} from {}", request.adapter_name, request.device_address);
                        Ok(HID_INFORMATION_BIN.into())
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            }, Characteristic {
                uuid: HID_CONTROL_POINT,
                write: Some(CharacteristicWrite { 
                    write_without_response: true,
                    method: CharacteristicWriteMethod::Io,
                    ..Default::default()
                }),
                control_handle: hid_control_point_handle,
                ..Default::default()
            }, Characteristic {
                uuid: HID_REPORT_MAP,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|request| Box::pin(async move {
                        println!("REPORT_MAP read by {} from {}", request.adapter_name, request.device_address);
                        Ok(REPORT_DESCRIPTOR.into())
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            }, Characteristic {
                uuid: HID_BOOT_KEYBOARD_INPUT,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|request| Box::pin(async move {
                        println!("BOOT_KEYBOARD_INPUT read by {} from {}", request.adapter_name, request.device_address);
                        Ok(vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                    })),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),
                control_handle: boot_keyboard_input_handle,
                ..Default::default()
            }, Characteristic {
                uuid: HID_BOOT_KEYBOARD_OUTPUT,
                read: Some(CharacteristicRead {
                    read: true,
                    ..Default::default()
                }),
                write: Some(CharacteristicWrite {
                    write: true,
                    write_without_response: true,
                    ..Default::default()
                }),
                ..Default::default()
            }, Characteristic {
                uuid: HID_REPORT,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(|request| 
                        Box::pin(async move {
                            println!("HID REPORT read by {} from {}", request.adapter_name, request.device_address);
                            Ok(vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
                        })
                    ),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),

                control_handle: input_report_handle,
                descriptors: vec![Descriptor {
                    uuid: REPORT_REFERENCE,
                    read: Some(DescriptorRead {
                        read: true,
                        fun: Box::new(|request| {
                            Box::pin(async move {
                                println!("Descirptor for HID_REPORT read by {}", request.device_address);
                                Ok([0x00, 0x01].into())
                            })
                        }),

                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],

            ..Default::default()
        }],
        ..Default::default()
    };
    let app_handle = adapter.serve_gatt_application(app).await?;

    println!("Press enter to quit");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    pin_mut!(hid_control_point_control);
    pin_mut!(boot_keyboard_input_control);
    pin_mut!(input_report_control);

    let mut writer: Option<CharacteristicWriter> = None;
    
    loop {
        tokio::select! {
            _ = lines.next_line() => break,
            _ = sleep(Duration::from_millis(250)) => {
                if let Some(writer) = &writer {
                    println!("A drown sent");
                    let _ = writer.send(&[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]).await;
                    sleep(Duration::from_millis(100)).await;
                    println!("A up sent");
                    let _ = writer.send(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]).await;
                }
            }
            event = hid_control_point_control.next() => {
                if let Some(CharacteristicControlEvent::Write(req)) = event {
                    println!("Write to hid control point from {} on {}", req.device_address(), req.adapter_name());
                    match req.accept() {
                        Ok(reader) => {
                            match reader.recv().await {
                                Ok(data) => println!("Written data: {:?}", data),
                                Err(error) => println!("Error reading data: {:?}", error)
                            };

                        },
                        Err(error) => println!("Error accepting read request: {:?}", error)
                    };

                }
            },
            event = boot_keyboard_input_control.next() => {
                if let Some(CharacteristicControlEvent::Notify(new_writer)) = event {
                    println!("Attaching notifier for boot keyboard input");
                    writer = Some(new_writer);
                }
                
            },
            event = input_report_control.next() => {
                if let Some(CharacteristicControlEvent::Notify(new_writer)) = event {
                    println!("Attaching notifier for keyboard input");
                    writer = Some(new_writer);
                }
            }
        }
    }

    println!("Removing advertisement and server");
    drop(handle);
    drop(app_handle);
    sleep(Duration::from_secs(1)).await;


    return Ok(())
}
async fn battery() -> bluer::Result<()> {
    env_logger::init();
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Advertising on Bluetooth adapter {} with address {}", adapter.name(), adapter.address().await?);
    let le_advertisement = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: vec!["0000180F-0000-1000-8000-00805F9B34FB".parse().unwrap()].into_iter().collect(),
        discoverable: Some(true),
        local_name: Some("le_advertise".to_string()),
        ..Default::default()
    };
    println!("{:?}", &le_advertisement);
    let handle = adapter.advertise(le_advertisement).await?;

    let percent = Arc::new(RwLock::new(3u8));
    let reader = percent.clone();

    println!("Serving GATT battery service on Bluetooth adapter {}", adapter.name());
    let (char_control, char_handle) = characteristic_control();
    let app = Application {
        services: vec![Service {
            uuid: "0000180F-0000-1000-8000-00805F9B34FB".parse().unwrap(),
            primary: true,
            characteristics: vec![Characteristic {
                uuid: "00002A19-0000-1000-8000-00805F9B34FB".parse().unwrap(),
                write: None,
                read: Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(move |_request| {
                        let re = reader.clone();
                        Box::pin(async move {
                            let value = *re.read().await;
                            Ok(vec![value])
                        })
                    }),
                    ..Default::default()
                }),
                notify: Some(CharacteristicNotify {
                    notify: true,
                    method: CharacteristicNotifyMethod::Io,
                    ..Default::default()
                }),
                
                control_handle: char_handle,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let app_handle = adapter.serve_gatt_application(app).await?;

    println!("Press enter to quit");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    let mut writer: Option<CharacteristicWriter> = None;

    pin_mut!(char_control);
    
    loop {
        tokio::select! {
            _ = lines.next_line() => break,
            mut per = sleep(Duration::from_millis(25)).then(|_| percent.write()) => match *per == 0 {
                true => {*per = 100; if let Some(ref writer) = writer { let _ = writer.send(&[*per]).await; };},
                false => {*per -= 1; if let Some(ref writer) = writer { let _ = writer.send(&[*per]).await; };}
            },
            event = char_control.next() => {
                match event {
                    Some(CharacteristicControlEvent::Notify(notifier)) => {
                        writer = Some(notifier);
                        println!("Registering listener")
                    },
                    _ => continue,
                }
            }
        }
    }

    println!("Removing advertisement and server");
    drop(handle);
    drop(app_handle);
    sleep(Duration::from_secs(1)).await;

    Ok(())
}

