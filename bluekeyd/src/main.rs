use std::{collections::{HashMap, hash_map::Entry}, ops::Deref, path::Path, str::FromStr, sync::Arc};

use bluer::Address;
use evdev::KeyCode;
use futures::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
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



enum Bridge {
    Keyboard(KeyboardBridge),
    Mouse(MouseBridge)
}
impl Bridge {
    async fn cancel(self) -> Result<(), EvdevBridgeError> {
        match self {
            Self::Keyboard(keyboard) => keyboard.cancel().await.map(|_| ()),
            Self::Mouse(mouse) => mouse.cancel().await
        }
    }
}


struct Bluekey {
    bridge_id_source: IdSource,
    bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>,
    bridge_broken_sender: mpsc::Sender<Id>,
    
    escape_shortcut: Shortcut,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>
}

struct LostBus;
struct DeadChannel;
impl Bluekey {
    async fn disconnection_bridge_cleaner(mut source: NameOwnerChangedStream, bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>, connection: Connection) -> Result<(), LostBus> {
        let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").unwrap();

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
                
                for (handle, (_, bridge)) in bridges.extract_if(|_, (name, _)| name == old_name) {
                    if let Err(error) = bridge.cancel().await {
                        warn!("Bridge with handle {} failed with error: {:?}", handle, error)
                    }
                    
                    emitter.bridge_broken(handle).await.unwrap();
                    info!("Cleaned handle {} from dead process", handle);
                }
                
            }
        }
    }
    async fn death_bridge_cleaner(mut source: mpsc::Receiver<Id>, bridges: Arc<Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>>, connection: Connection) -> Result<(), DeadChannel> {
        let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").unwrap();

        loop {
            let bridge = source.recv().await.ok_or(DeadChannel)?;
            let mut bridges = bridges.lock().await;

            let entry = match bridges.entry(bridge) {
                Entry::Vacant(_) => continue,
                Entry::Occupied(entry) => entry
            };

            if let Err(error) = entry.remove().1.cancel().await {
                warn!("Bridge with handle {} failed with error: {:?}", bridge, error);
            }
            
            emitter.bridge_broken(bridge).await.unwrap();
            info!("Cleaned handle {} from escape or error", bridge);
        }
    }


    async fn start(keyboard: Keyboard, mouse: Mouse) -> Result<Connection, zbus::Error> {
        let bridges = Arc::new(Mutex::new(HashMap::new()));
        let (sender, reciever) = mpsc::channel(16);
        
        let connection = {
            let server = Self {
                bridge_id_source: IdSource::new(),
                bridges: bridges.clone(),
                bridge_broken_sender: sender,

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

        tokio::spawn(Self::disconnection_bridge_cleaner(source, bridges.clone(), connection.clone()));
        tokio::spawn(Self::death_bridge_cleaner(reciever, bridges.clone(), connection.clone()));
       
        Ok(connection)
    }
}

#[interface(name = "us.colbystuff.Bluekey.Bridge1")]
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
        
        // Acquire ID and store bridge
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

        // Acquire ID and start the bridge
        let id = self.bridge_id_source.next();
        let target = self.bridge_broken_sender.clone();

        let bridge = KeyboardBridge::start(
            self.keyboard_server.clone(), 
            device.into_event_stream().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?, 
            Target::Target(mac),
            self.escape_shortcut.clone(),
            async move || {target.send(id).await.unwrap()}
        );        
        
        // Store the bridge
        self.bridges.lock().await.insert(id, (name.into(), Bridge::Keyboard(bridge)));

        info!("Started keyboard bridge from {} to {} with handle {}", keyboard.display(), mac, id);
        Ok(id)
    }
    async fn destroy_bridge(&mut self, #[zbus(header)] header: Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, handle: Id) -> Result<(), zbus::fdo::Error> {
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
        }
            
        emitter.bridge_broken(handle).await.unwrap();
        info!("Destroyed bridge with handle {}", handle);

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