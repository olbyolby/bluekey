use std::{collections::{HashMap, hash_map::Entry}, fmt::Write, sync::Arc};

use bluer::{Adapter, AdapterEvent, Address};
use blueshare::bluetooth::{ReturnError, keyboard::{Keyboard, KeyboardReturnEvent}, mouse::{Mouse, MouseReturnEvent}};
use futures::StreamExt;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use zbus::{Connection, interface, object_server::{InterfaceRef, SignalEmitter}};


#[derive(Deserialize, Serialize, zvariant::Type, zvariant::Value, PartialEq, Eq, Debug, Clone, Copy)]
#[repr(u8)]
enum PowerStatus {
    Suspended = 1,
    Active = 0,
}

struct Device {
    address: String,
    power: PowerStatus,
    has_keyboard: bool,
    has_mouse: bool
}
impl Device {
    fn new(address: String) -> Self {
        Self {
            address,
            power: PowerStatus::Active,
            has_keyboard: false,
            has_mouse: false
        }
    }

    async fn set_keyboard(&mut self, value: bool, emitter: &SignalEmitter<'_>) -> Result<(), zbus::Error> {
        self.has_keyboard = value;
        self.has_keyboard_changed(emitter).await
    }
    async fn set_mouse(&mut self, value: bool, emitter: &SignalEmitter<'_>) -> Result<(), zbus::Error> {
        self.has_mouse = value;
        self.has_mouse_changed(emitter).await
    }
    async fn set_power(&mut self, value: PowerStatus, emitter: &SignalEmitter<'_>) -> Result<(), zbus::Error> {
        self.power = value;
        self.power_changed(emitter).await
    }
}

#[interface(name = "us.colbystuff.Bluekey.Device1")]
impl Device {
    #[zbus(property(emits_changed_signal = "const"))]
    fn address(&self) -> &str {
        &self.address
    }

    #[zbus(property)]
    fn power(&self) -> PowerStatus {
        self.power
    }

    #[zbus(property)]
    fn has_keyboard(&self) -> bool {
        self.has_keyboard
    }

    #[zbus(property)]
    fn has_mouse(&self) -> bool {
        self.has_mouse
    }
}

struct DeviceMap {
    devices: HashMap<Address, String>,
    connection: Connection
}
impl DeviceMap {
    fn new(connection: Connection) -> Self {
        Self { devices: HashMap::new(), connection }
    }

    async fn acquire(&mut self, address: Address) -> Result<(InterfaceRef<Device>, &str), zbus::Error>{
        let path = &**match self.devices.entry(address) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let device = Device::new(address.to_string());
                let path = Self::address_path(address);

                info!("Created device {}", path);
                self.connection.object_server().at(&*path, device).await?;
                entry.insert(path)
            } 
        };

        self.connection.object_server().interface(path).await
            .map(|interface| (interface, path))

    }

    fn address_path(address: Address) -> String {
        let mut path = String::from("/us/colbystuff/Bluekey/devices/");
        for part in address.iter() {
            write!(path, "{:02x}", part).unwrap();
        }

        path
    }

    async fn remove(&mut self, address: Address) {
        if let Some(path) = self.devices.remove(&address) {
            info!("Removed device {}", path);
            if let Err(error) = self.connection.object_server().remove::<Device, _>(&*path).await {
                warn!("Error removing device {}: {}", path, error)
            }
        }
    }

    
}

pub async fn devices_tracker(connection: Connection, adapter: Arc<Adapter>, keyboard: Arc<Keyboard>, mouse: Arc<Mouse>) -> Result<(), ReturnError> {
    let mut keyboard = keyboard.listen();
    let mut mouse = mouse.listen();
    let mut adapter_events = adapter.events().await.unwrap();

    connection.object_server().at("/us/colbystuff/Bluekey/devices", zbus::fdo::ObjectManager).await.unwrap();
    
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Notification {
        KeyboardAvailable,
        MouseAvailable,
        Wake,
        Suspend
    }

    let mut devices = DeviceMap::new(connection.clone());

    loop {
        let (address, notification) = tokio::select! {
            event = keyboard.next_event() => match event? {
                KeyboardReturnEvent::Register(address) => (address, Notification::KeyboardAvailable),
                KeyboardReturnEvent::Suspend(address) => (address, Notification::Suspend),
                KeyboardReturnEvent::Wake(address) => (address, Notification::Wake),
                _ => continue
            },
            event = mouse.next_event() => match event? {
                MouseReturnEvent::Register(address) => (address, Notification::MouseAvailable),
                MouseReturnEvent::Suspend(address) => (address, Notification::Suspend),
                MouseReturnEvent::Wake(address) => (address, Notification::Wake),
            },
            event = adapter_events.next() => match event {
                Some(AdapterEvent::DeviceRemoved(address)) => {
                    devices.remove(address).await;
                    continue
                },
                _ => continue
            }
        };

        let (interface, path) = match devices.acquire(address).await {
            Ok(interface) => interface,
            Err(error) => {
                warn!("Error creating device object for {}: {}", address, error);
                continue;
            }
        };
        
        let mut device = interface.get_mut().await;
        let result = match notification {
            Notification::KeyboardAvailable => device.set_keyboard(true, interface.signal_emitter()).await,
            Notification::MouseAvailable => device.set_mouse(true, interface.signal_emitter()).await,
            Notification::Suspend => device.set_power(PowerStatus::Suspended, interface.signal_emitter()).await,
            Notification::Wake => device.set_power(PowerStatus::Active, interface.signal_emitter()).await            
        };
        if let Err(error) = result {
            warn!("Error sending {:?} notification to {}: {}", notification, path, error)
        }
        
    }

}