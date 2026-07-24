#![allow(dead_code)] // Annoying 'cause I have unfinished APIs
// A proof of concept daemon for my Bluetooth keyboard and mosue emulator
use std::{collections::{HashMap, hash_map::Entry}, ops::Deref, path::Path, str::FromStr, sync::Arc};

use bluer::Address;
use futures::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use zbus::{interface, message::Header, names::OwnedUniqueName};

use blueshare::bluetooth::{Target, keyboard::Keyboard, mouse::Mouse};

use blueshare::evdev_bridge::{EvdevBridgeError, KeyboardBridge, MouseBridge};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
struct Id(u64);
impl zvariant::Type for Id {
    const SIGNATURE: &'static zvariant::Signature = &zvariant::signature!("(t)");
}
impl std::fmt::Display for Id {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "<{}>", self.0)?;
        Ok(())
    }
}


struct IdSource {
    id: u64
}
impl IdSource {
    fn new() -> Self {
        Self { id: 0 }
    }
    fn next(&mut self) -> Id {
        let id = self.id;
        self.id += 1;
        Id(id)
    }
}

enum Bridge {
    Keyboard(KeyboardBridge),
    Mouse(MouseBridge)
}
impl Bridge {
    async fn cancel(self) -> Result<(), EvdevBridgeError> {
        match self {
            Self::Keyboard(board) => board.cancel().await,
            Self::Mouse(mouse) => mouse.cancel().await
        }
    }
}



struct Bluekey {
    bridge_id_source: IdSource,
    bridges: Arc<RwLock<HashMap<Id, (OwnedUniqueName, Bridge)>>>,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>
}

impl Bluekey {
    fn dbus_shandler(&self) {
        
    }
}

#[interface(name = "us.colbystuff.Bluekey1")]
impl Bluekey {
    async fn bridge_mouse(&mut self, #[zbus(header)] header: Header<'_>, mouse: &Path, mac: &str) -> Result<Id, zbus::fdo::Error> {
        let name = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("No unique sender name".into()))?.clone();
        let mac = Address::from_str(mac).map_err(|_| zbus::fdo::Error::InvalidArgs("Invalid MAC address".into()))?;

        // Open and grab the device
        let mut device = evdev::Device::open(mouse).map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
        device.grab().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;

        // Start the bridge
        let bridge = MouseBridge::start(
            self.mouse_server.clone(), 
            device.into_event_stream().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?, 
            Target::Target(mac)
        );
        
        // Acquired ID and store bridge
        let id = self.bridge_id_source.next();
        self.bridges.write().await.insert(id, (name.into(), Bridge::Mouse(bridge)));
        

        info!("Started mouse bridge from {} to {} with handle {}", mouse.display(), mac, id);
        Ok(id)
    }
    async fn bridge_keyboard(&mut self, #[zbus(header)] header: Header<'_>, keyboard: &Path, mac: &str) -> Result<Id, zbus::fdo::Error> {
        let name = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("No unique sender name".into()))?.clone();
        let mac = Address::from_str(mac).map_err(|_| zbus::fdo::Error::InvalidArgs("Invalid MAC address".into()))?;

        // Open and grab the device
        let mut device = evdev::Device::open(keyboard).map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
        device.grab().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;

        // Start the bridge
        let bridge = KeyboardBridge::start(
            self.keyboard_server.clone(), 
            device.into_event_stream().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?, 
            Target::Target(mac)
        );
        
        // Acquired ID and store bridge
        let id = self.bridge_id_source.next();
        self.bridges.write().await.insert(id, (name.into(), Bridge::Keyboard(bridge)));

        info!("Started keyboard bridge from {} to {} with handle {}", keyboard.display(), mac, id);
        Ok(id)
    }
    async fn destroy_bridge(&mut self, #[zbus(header)] header: Header<'_>, handle: Id) -> Result<(), zbus::fdo::Error> {
        let name = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("No unique sender name".into()))?.clone();

        let mut bridges = self.bridges.write().await;
        let entry = match bridges.entry(handle) {
            Entry::Vacant(_) => Err(zbus::fdo::Error::Failed("No such handle".into())),
            Entry::Occupied(entry) => Ok(entry)
        }?;

        if name != entry.get().0 {
            return Err(zbus::fdo::Error::AccessDenied("Invalid handle".into()))
        }

        if let Err(error) = entry.remove().1.cancel().await {
            warn!("Bridge with handle {} failed with error: {:?}", handle, error);
        };

        info!("Destoryed bridge with handle {}", handle);
        Ok(())
    }

}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), zbus::Error> {
    env_logger::init();
    debug!("Test log");

    let session = bluer::Session::new().await.unwrap();
    let adapter = Arc::new(session.default_adapter().await.unwrap());

    let bridges = Arc::new(RwLock::new(HashMap::new()));
    let test = Bluekey { bridge_id_source: IdSource::new(), bridges: bridges.clone(), keyboard_server: Arc::new(Keyboard::new(adapter.clone())), mouse_server: Arc::new(Mouse::new(adapter.clone()))};
    let connection = zbus::connection::Builder::session()?.name("us.colbystuff.Bluekey")?.serve_at("/us/colbystuff/Bluekey",test)?.build().await?;

    let dbus = zbus::fdo::DBusProxy::new(&connection).await.unwrap();
    let mut owner_change = dbus.receive_name_owner_changed().await.unwrap();
    
    while let Some(change) = owner_change.next().await {
        let args = change.args().unwrap();

        let name = match args.old_owner().deref() {
            Some(name) => name,
            None => continue
        };
        if args.new_owner == None.into() {
            let mut bridges = bridges.write().await;
            bridges.retain(|_, (owner, _)| owner!=name)
        }
    }
    

    drop(connection);
    Ok(())
}