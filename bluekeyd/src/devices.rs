use std::{collections::{HashMap, hash_map::Entry}, ops::Deref, sync::{Arc, atomic::AtomicBool}, fmt::Write};

use bluer::{Adapter, AdapterEvent, Address};
use blueshare::bluetooth::{ReturnError, keyboard::{Keyboard, KeyboardReturnEvent}, mouse::{Mouse, MouseReturnEvent}};
use futures::StreamExt;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use zbus::{Connection, interface};


#[derive(Deserialize, Serialize, zvariant::Type, zvariant::Value, PartialEq, Eq, Debug, Clone, Copy)]
#[repr(u8)]
enum PowerStatus {
    Suspended,
    Active,
}





struct Device {
    address: String,
    suspended: AtomicBool,
    has_keyboard: AtomicBool,
    has_mouse: AtomicBool
}
impl Device {
    fn new(address: String) -> Self {
        Self {
            address,
            suspended: AtomicBool::new(false),
            has_keyboard: AtomicBool::new(false),
            has_mouse: AtomicBool::new(false)
        }
    }
}
#[derive(Clone)]
struct DeviceInterface(Arc<Device>);
impl Deref for DeviceInterface {
    type Target = Device;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[interface(name = "us.colbystuff.Bluekey.Device1")]
impl DeviceInterface {
    #[zbus(property(emits_changed_signal = "const"))]
    fn address(&self) -> &str {
        &self.address
    }

    #[zbus(property)]
    fn status(&self) -> PowerStatus {
        match self.suspended.load(std::sync::atomic::Ordering::Acquire) {
            true => PowerStatus::Suspended,
            false => PowerStatus::Active
        }
    }

    #[zbus(property)]
    fn has_keyboard(&self) -> bool {
        self.has_keyboard.load(std::sync::atomic::Ordering::Acquire)
    }

    #[zbus(property)]
    fn has_mouse(&self) -> bool {
        self.has_mouse.load(std::sync::atomic::Ordering::Acquire)
    }
}

struct DeviceMap {
    devices: HashMap<Address, Arc<Device>>,
    connection: Connection
}
impl DeviceMap {
    fn new(connection: Connection) -> Self {
        Self { devices: HashMap::new(), connection }
    }

    async fn acquire(&mut self, address: Address) -> Result<Arc<Device>, zbus::Error> {
        match self.devices.entry(address) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let device = Arc::new(Device::new(address.to_string()));

                info!("Created device {}", Self::address_path(address));
                self.connection.object_server().at(Self::address_path(address), DeviceInterface(device.clone())).await?;
                Ok(entry.insert(device.clone()).clone())
            } 
        }
    }

    fn address_path(address: Address) -> String {
        let mut path = String::from("/us/colbystuff/Bluekey/devices/");
        for part in address.iter() {
            write!(path, "{:02x}", part).unwrap();
        }

        path
    }

    async fn remove(&mut self, address: Address) {
        if let Some(_) = self.devices.remove(&address) {
            info!("Removed device {}", Self::address_path(address));
            self.connection.object_server().remove::<DeviceInterface, _>(Self::address_path(address)).await.unwrap();
        }
    }

    
}

pub async fn devices_tracker(connection: Connection, adapter: Arc<Adapter>, keyboard: Arc<Keyboard>, mouse: Arc<Mouse>) -> Result<(), ReturnError> {
    let mut keyboard = keyboard.listen();
    let mut mouse = mouse.listen();
    let mut adapter_events = adapter.events().await.unwrap();

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum DeviceType {
        Keyboard,
        Mouse
    }

    // This is a mess of what can only be described as "ownership spegehtti", due to the fact zbus really wants an interface to be responsible for itself and it's own updates, but, in this case, it simply... isn't.
    // I'm half convinced it'd be better to just handle all these messages by hand or something. 
    let mut devices = DeviceMap::new(connection.clone());

    loop {
        let (address, device_type) = tokio::select! {
            event = keyboard.next_event() => match event? {
                KeyboardReturnEvent::Register(event) => (event, DeviceType::Keyboard),
                _ => continue
            },
            event = mouse.next_event() => match event? {
                MouseReturnEvent::Register(event) => (event, DeviceType::Mouse),
                _ => continue
            },
            event = adapter_events.next() => match event {
                Some(AdapterEvent::DeviceRemoved(address)) => {
                    devices.remove(address).await;
                    continue
                },
                _ => continue
            }
        };

        let device = match devices.acquire(address).await {
            Ok(device) => device,
            Err(error) => {
                warn!("Error creating device object for {}: {}", address, error);
                continue;
            }
        };
        
        use std::sync::atomic::Ordering;
        let interface = connection.object_server().interface::<_, DeviceInterface>(DeviceMap::address_path(address)).await.unwrap();
        match device_type {
            // You know, the whole point of atomics was to NOT lock things, but, for some bloody reason. property change signals lock. Why? Who knows?!
            DeviceType::Keyboard => {
                device.has_keyboard.store(true, Ordering::Release);
                interface.get().await.has_keyboard_changed(interface.signal_emitter()).await.unwrap();
            },
            DeviceType::Mouse => {
                device.has_mouse.store(true, Ordering::Release);
                interface.get().await.has_mouse_changed(interface.signal_emitter()).await.unwrap();
            }
        }
        
    }

}