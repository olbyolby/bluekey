// A proof of concept daemon for my Bluetooth keyboard and mosue emulator
use std::{collections::{HashMap, hash_map::Entry}, ops::Deref, path::Path, str::FromStr, sync::Arc};

use bluer::Address;
use evdev::KeyCode;
use futures::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use zbus::{Connection, fdo::NameOwnerChangedStream, interface, message::Header, names::OwnedUniqueName, object_server::SignalEmitter};

use blueshare::{bluetooth::{Target, keyboard::Keyboard, mouse::Mouse}, evdev_bridge::Shortcut};
use blueshare::evdev_bridge::{EvdevBridgeError, KeyboardBridge, MouseBridge};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
struct Id(u64);
impl zvariant::Type for Id {
    const SIGNATURE: &'static zvariant::Signature = &zvariant::signature!("t");
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



struct Bridge {
    canceller: Option<oneshot::Sender<()>>
}
impl Bridge {
    // There's got to be a better way to do this that I just don't know
    // Slight memory leak: A handle isn't removed from the birdge map until it's connection closes
    // Won't fix until a better solution to this spegehti is found.
    fn new(connection: Connection, id: Id, value: impl Future<Output=Result<(), EvdevBridgeError>> + Send + 'static) -> Self {
        let (canceller, cancellee) = oneshot::channel();
        tokio::spawn(async move {
            let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").unwrap();
            
            let error = tokio::select! {
                _ = cancellee => None,
                value = value => value.err()
            };
            if let Some(error) = error {
                warn!("Bridge with handle {} failed with error: {:?}", id, error);
            }

            emitter.bridge_broken(id).await.unwrap();
        });
        
        Self {
            canceller: Some(canceller)
        }
    }

    fn destroy(self) {
        drop(self);
    }
}
 
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(()); // Nothing to cancel if receiver is dropped
        }
    }
}


struct Bluekey {
    bridge_id_source: IdSource,
    bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>,
    
    escape_shortcut: Shortcut,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>
}

struct LostBus;
impl Bluekey {
    async fn bridge_cleaner(mut source: NameOwnerChangedStream, bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>) -> Result<(), LostBus> {
       
        loop {
            let change = source.next().await.ok_or(LostBus)?;

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
                
                for (handle, _) in bridges.extract_if(|_, (name, _)| name == old_name) {
                    info!("Cleaned handle {:?}", handle)
                }
                
            }
        }
    }

    async fn start(keyboard: Keyboard, mouse: Mouse) -> Result<Connection, zbus::Error> {
        let bridges = Arc::new(Mutex::new(HashMap::new()));
        
        let connection = {
            let server = Self {
                bridge_id_source: IdSource::new(),
                bridges: bridges.clone(),

                escape_shortcut: Shortcut::new(Arc::new(Vec::from([KeyCode(1), KeyCode(125)]))),

                keyboard_server: Arc::new(keyboard),
                mouse_server: Arc::new(mouse),
                
            };

            zbus::connection::Builder::session()?
                .name("us.colbystuff.Bluekey")?
                .serve_at("/us/colbystuff/Bluekey", server)?
                .build()
                .await?
        };
        
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        let source = dbus.receive_name_owner_changed().await?;

        tokio::spawn(Self::bridge_cleaner(source, bridges.clone()));
       
        Ok(connection)
    }
}

#[interface(name = "us.colbystuff.Bluekey.Bridge1")]
impl Bluekey {
    async fn bridge_mouse(&mut self, #[zbus(header)] header: Header<'_>, #[zbus(connection)] connection: &Connection, mouse: &Path, mac: &str) -> Result<Id, zbus::fdo::Error> {
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
        
        // Acquire ID and store bridge
        let id = self.bridge_id_source.next();
        self.bridges.lock().await.insert(id, (name.into(), Bridge::new(connection.clone(), id, bridge.wait_for_break())));
        
        
        info!("Started mouse bridge from {} to {} with handle {}", mouse.display(), mac, id);
        Ok(id)
    }
    async fn bridge_keyboard(&mut self, #[zbus(header)] header: Header<'_>, #[zbus(connection)] connection: &Connection, keyboard: &Path, mac: &str) -> Result<Id, zbus::fdo::Error> {
        let name = header.sender().ok_or_else(|| zbus::fdo::Error::Failed("No unique sender name".into()))?.clone();
        let mac = Address::from_str(mac).map_err(|_| zbus::fdo::Error::InvalidArgs("Invalid MAC address".into()))?;

        // Open and grab the device
        let mut device = evdev::Device::open(keyboard).map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
        device.grab().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;

        // Start the bridge
        let bridge = KeyboardBridge::start(
            self.keyboard_server.clone(), 
            device.into_event_stream().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?, 
            Target::Target(mac),
            self.escape_shortcut.clone()
        );
        
        
        // Acquire ID and store bridge
        let id = self.bridge_id_source.next();
        self.bridges.lock().await.insert(id, (name.into(), Bridge::new(connection.clone(), id, bridge.wait_for_break())));

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

        entry.remove().1.destroy();


        
        info!("Destoryed bridge with handle {}", handle);
        Ok(())
    }

    #[zbus(signal)]
    async fn bridge_broken(emitter: &SignalEmitter<'_>, bridge: Id) -> Result<(), zbus::Error>;

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