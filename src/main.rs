// Example of emulating a bluetooth device, makes a fake bluetooth battery
use bluer::{adv::Advertisement, gatt::{CharacteristicWriter, local::{Application, Characteristic, CharacteristicControlEvent, CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead, Service, characteristic_control}}};

use futures::{FutureExt, StreamExt, pin_mut};
use std::{sync::{Arc}, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::sleep,
    sync::RwLock
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
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

    println!("Serving GATT echo service on Bluetooth adapter {}", adapter.name());
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