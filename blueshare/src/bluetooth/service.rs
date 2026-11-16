// A single HID over GATT service carrying both the keyboard(Report ID 1) and the mouse(Report ID 2).
//
// Bluekey used to register the keyboard and the mouse as two separate HID services. Linux and Windows
// hosts bind every HID service a device has, but Android's HID host only binds the *first* HID service
// it discovers on a device and ignores the rest, so Android phones ended up with only the mouse(or only
// the keyboard). A composite service with one Report Map and one Report characteristic per report,
// like real keyboard + touchpad combos use, works everywhere.
use std::sync::OnceLock;

use bluer::{
    Adapter, AdapterEvent, AdapterProperty,
    adv::{Advertisement, AdvertisementHandle},
    gatt::local::{Application, ApplicationHandle, ReqError, Service}
};
use futures::{StreamExt, pin_mut};
use log::{debug, info, warn};

use super::{
    hid::{self, Protocol, characteristics::callback},
    keyboard::{self, Keyboard, KeyboardReturnEvent},
    mouse::{self, Mouse, MouseReturnEvent}
};

// GAP appearance: Keyboard
const APPEARANCE: u16 = 0x03C1;

// Keeps the HID service and its advertisement registered for as long as it's alive
pub struct HidService {
    _application: ApplicationHandle,
    advertiser: tokio::task::JoinHandle<()>
}
impl Drop for HidService {
    fn drop(&mut self) {
        self.advertiser.abort();
    }
}

// Advertise with a name: phones(Android in particular) hide nameless LE devices from their Bluetooth scan,
// and pairing from the phone's own scan is what bonds it over LE, the transport HID over GATT uses
fn advertisement(name: String) -> Advertisement {
    Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: [hid::definitions::SERVICE].into(),
        discoverable: Some(true),
        appearance: Some(APPEARANCE),
        local_name: Some(name),

        ..Default::default()
    }
}

// Powering the adapter off(btmgmt/bluetoothctl power off, toggling BR/EDR, suspending the laptop) makes the
// kernel drop every advertising instance, and BlueZ doesn't put registered advertisements back on power on.
// Without one, a bonded phone has nothing to reconnect to, so register it again every time the adapter powers on.
async fn keep_advertising(adapter: Adapter, name: String, first: AdvertisementHandle) {
    let mut current = Some(first);
    let events = match adapter.events().await {
        Ok(events) => events,
        Err(error) => {
            warn!("Can't watch the Bluetooth adapter({}), advertising won't come back after it powers off", error);
            std::future::pending::<()>().await;
            return
        }
    };
    pin_mut!(events);
    while let Some(event) = events.next().await {
        if let AdapterEvent::PropertyChanged(AdapterProperty::Powered(true)) = event {
            // The old registration's kernel instance is already gone, drop it before registering a new one
            current = None;
            for attempt in 1..=5 {
                match adapter.advertise(advertisement(name.clone())).await {
                    Ok(handle) => {
                        info!("Bluetooth adapter powered on, advertising again so the phone can reconnect");
                        current = Some(handle);
                        break
                    },
                    Err(error) => {
                        warn!("Re-advertising after power on failed(attempt {}/5): {}", attempt, error);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
    drop(current);
}

// The keyboard and mouse report descriptors, each with its own Report ID, back to back in one Report Map
fn report_map() -> &'static [u8] {
    static REPORT_MAP: OnceLock<Vec<u8>> = OnceLock::new();
    REPORT_MAP.get_or_init(|| [keyboard::data::REPORT_DESCRIPTOR, mouse::data::REPORT_DESCRIPTOR].concat())
}

pub async fn serve(adapter: &Adapter, keyboard: &Keyboard, mouse: &Mouse) -> Result<HidService, bluer::Error> {
    let keyboard_state = keyboard.state.clone();
    let mouse_state = mouse.state.clone();
    let keyboard_returns = keyboard.returns_sender.clone();
    let mouse_returns = mouse.returns_sender.clone();

    // Characteristics that may only exist once per HID service, shared by the keyboard and the mouse
    let mut characteristics = vec![
        hid::characteristics::protocol_mode(
            callback!(|request| keyboard_state {
                debug!("Read protocol by {}", request.device_address);
                let protocol = match keyboard_state.upgrade() {
                    Some(state) => {
                        let state = state.read().await;
                        state.get_device(request.device_address).map(|device| device.protocol)
                    },
                    None => None
                };
                Ok(vec![protocol.unwrap_or(Protocol::Report).into()])
            }),
            callback!(|value, request| keyboard_state, mouse_state {
                debug!("Write protocol by {} with {:?}", request.device_address, value);
                let protocol = match value.first() {
                    Some(0) => Protocol::Boot,
                    Some(1) => Protocol::Report,
                    _ => return Ok(())
                };
                // Protocol mode applies to the whole service, so to both the keyboard and the mouse
                if let Some(state) = keyboard_state.upgrade() {
                    state.write().await.acquire_device(request.device_address).await.protocol = protocol;
                }
                if let Some(state) = mouse_state.upgrade() {
                    state.write().await.acquire_device(request.device_address).await.protocol = protocol;
                }
                Ok(())
            })
        ),
        hid::characteristics::information(hid::HID_INFORMATION),
        hid::characteristics::control_point(callback!(|value, request| keyboard_returns, mouse_returns {
            debug!("Write control point by {} with {:?}", request.device_address, value);
            let address = request.device_address;
            let (keyboard_event, mouse_event) = match value.first() {
                Some(0) => (KeyboardReturnEvent::Suspend(address), MouseReturnEvent::Suspend(address)),
                Some(1) => (KeyboardReturnEvent::Wake(address), MouseReturnEvent::Wake(address)),
                _ => return Err(ReqError::Failed)
            };
            let _ = keyboard_returns.send(keyboard_event);
            let _ = mouse_returns.send(mouse_event);
            Ok(())
        })),
        // A host reading the report map is setting this up as its keyboard and mouse, so register it with both(this
        // is what makes it show up as "Keyboard, Mouse" in `bluekey list`, notification sessions carry no address)
        hid::characteristics::report_map(callback!(|request| keyboard_state, mouse_state {
            debug!("Report map read by {}", request.device_address);
            if let Some(state) = keyboard_state.upgrade() {
                state.write().await.acquire_device(request.device_address).await;
            }
            if let Some(state) = mouse_state.upgrade() {
                state.write().await.acquire_device(request.device_address).await;
            }
            Ok(report_map().to_vec())
        })),
    ];
    characteristics.extend(keyboard.take_characteristics());
    characteristics.extend(mouse.take_characteristics());

    let application = adapter.serve_gatt_application(Application {
        services: vec![Service {
            uuid: hid::definitions::SERVICE,
            primary: true,
            characteristics,
            ..Default::default()
        }],
        ..Default::default()
    }).await?;

    let name = adapter.alias().await.unwrap_or_else(|_| "Bluekey".to_string());
    let first = adapter.advertise(advertisement(name.clone())).await?;
    let advertiser = tokio::spawn(keep_advertising(adapter.clone(), name, first));

    info!("Serving combined keyboard + mouse HID service");
    Ok(HidService { _application: application, advertiser })
}
