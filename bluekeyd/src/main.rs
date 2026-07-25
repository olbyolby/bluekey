// A proof of concept daemon for my Bluetooth keyboard and mosue emulator
use std::{collections::{HashMap, hash_map::Entry}, ops::Deref, path::Path, str::FromStr, sync::Arc};

use bluer::Address;
use futures::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use zbus::{Connection, fdo::NameOwnerChangedStream, interface, message::Header, names::OwnedUniqueName};

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


struct Cancel;
struct Bluekey {
    bridge_id_source: IdSource,
    bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>,
    _bridge_cleaner: oneshot::Sender<Cancel>
}

struct LostBus;
impl Bluekey {
    async fn bridge_cleaner(mut cancel: oneshot::Receiver<Cancel>, mut source: NameOwnerChangedStream, bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>) -> Result<(), LostBus> {
        let mut next = async || {
            tokio::select!{
                source = source.next() => Some(source.ok_or(LostBus)),
                _ = &mut cancel => None
            }.transpose()
        };
        
        while let Some(change) = next().await? {
            let (old_name, new_name) = match change.args() {
                Ok(args) => (args.old_owner, args.new_owner),
                Err(_) => continue
            };
            let old_name = match old_name.deref() {
                Some(old_name) => old_name,
                None => continue
            };

            if *new_name.deref() == None {
                let mut bridges = bridges.lock().await;

                bridges.retain(|_, (name, _)| name != old_name)
            }

        }

        Ok(())
    }

    async fn start(keyboard: Keyboard, mouse: Mouse) -> Result<Connection, zbus::Error> {
        let bridges = Arc::new(Mutex::new(HashMap::new()));
        let (canceller, cancelled) = oneshot::channel();
        
        let connection =  {
            let server = Self {
                bridge_id_source: IdSource::new(),
                bridges: bridges.clone(),

                keyboard_server: Arc::new(keyboard),
                mouse_server: Arc::new(mouse),
                _bridge_cleaner: canceller
            };
            zbus::connection::Builder::session()?
                .name("us.colbystuff.Bluekey")?
                .serve_at("/us/colbystuff/Bluekey", server)?
                .build()
                .await?
        };  
        
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        let source = dbus.receive_name_owner_changed().await?;
        tokio::spawn(Self::bridge_cleaner(cancelled, source, bridges.clone()));
        
        Ok(connection)
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
        self.bridges.lock().await.insert(id, (name.into(), Bridge::Mouse(bridge)));
        

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
        
        // Acquire ID and store bridge
        let id = self.bridge_id_source.next();
        self.bridges.lock().await.insert(id, (name.into(), Bridge::Keyboard(bridge)));

        info!("Started keyboard bridge from {} to {} with handle {}", keyboard.display(), mac, id);
        Ok(id)
    }
    async fn destroy_bridge(&mut self, #[zbus(header)] header: Header<'_>, handle: Id) -> Result<(), zbus::fdo::Error> {
        let name = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("No unique sender name".into()))?.clone();

        let mut bridges = self.bridges.lock().await;
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

    let keyboard = Keyboard::new(adapter.clone());
    let mouse = Mouse::new(adapter);
    let connection = Bluekey::start(keyboard, mouse).await?;
    
    std::future::pending::<()>().await;    
    drop(connection);
    
    Ok(())
}