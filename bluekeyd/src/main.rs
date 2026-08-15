use std::{
    collections::{
        HashMap, 
        hash_map::Entry
    }, fmt::Pointer, ops::Deref, path::Path, str::FromStr, sync::Arc,
};

use never_say_never::Never;
use bluer::{Adapter, AdapterEvent, Address};
use evdev::KeyCode;
use futures::{Stream, StreamExt, pin_mut};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::{select, sync::{Mutex, mpsc}};
use zbus::{
    Connection, 
    fdo::NameOwnerChanged, 
    interface, 
    message::Header, 
    names::OwnedUniqueName, 
    object_server::SignalEmitter
};

use blueshare::{
    bluetooth::{
        Target, 
        keyboard::Keyboard, 
        mouse::Mouse
    },
    evdev_bridge::{
        SharedShortcut, 
        Shortcut,
        EvdevBridgeError,
        KeyboardBridge,
        MouseBridge
    }
};



mod devices;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize, zvariant::Type)]
#[serde(transparent)]
struct Id(u64);
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
    Keyboard(KeyboardBridge, Address),
    Mouse(MouseBridge, Address)
}
impl Bridge {
    async fn cancel(self) -> Result<(), EvdevBridgeError> {
        match self {
            Self::Keyboard(keyboard, _) => keyboard.cancel().await.map(|_| ()),
            Self::Mouse(mouse, _) => mouse.cancel().await
        }
    }
    fn target(&self) -> Address {
        match self {
            Self::Keyboard(_, address) => *address,
            Self::Mouse(_, address) => *address
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
enum CleanerError {
    EmitterError(zbus::Error),
    DbusError(zbus::Error),
    BluetoothError(bluer::Error),
    StreamError
}
impl From<bluer::Error> for CleanerError {
    fn from(value: bluer::Error) -> Self {
        Self::BluetoothError(value)
    }
}
impl From<zbus::Error> for CleanerError {
    fn from(value: zbus::Error) -> Self {
        Self::DbusError(value)
    }
}

enum BluekeyError {
    Cleaner(CleanerError),
    DeviceTracker(devices::DeviceTrackerError)
}
impl From<CleanerError> for BluekeyError {
    fn from(value: CleanerError) -> Self {
        Self::Cleaner(value)
    }
}
impl From<devices::DeviceTrackerError> for BluekeyError {
    fn from(value: devices::DeviceTrackerError) -> Self {
        Self::DeviceTracker(value)
    }
}
impl std::fmt::Display for BluekeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cleaner(c) => c.fmt(f),
            Self::DeviceTracker(d) => d.fmt(f)
        }
    }
}

fn unwrap<T>(result: Result<Never, T>) -> T {
    match result {
        Err(e) => e
    }
}

type Bridges = Mutex<HashMap<Id, (OwnedUniqueName, Bridge)>>;
struct Bluekey {
    bridge_id_source: IdSource,
    bridges: Arc<Bridges>,
    bridge_broken_sender: mpsc::Sender<Id>,
    
    escape_shortcut: SharedShortcut,

    keyboard_server: Arc<Keyboard>,
    mouse_server: Arc<Mouse>
}
impl Bluekey {
    async fn disconnection_bridge_cleaner(source: impl Stream<Item=NameOwnerChanged>, bridges: Arc<Bridges>, connection: Connection) -> Result<Never, CleanerError> {
        let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").map_err(|e| CleanerError::EmitterError(e))?;
        pin_mut!(source);

        loop {
            let change = source.next().await.ok_or(CleanerError::StreamError)?;

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
                    
                    emitter.bridge_broken(handle).await?;
                    info!("Cleaned handle {} from dead process", handle);
                }
                
            }
        }
    }
    async fn death_bridge_cleaner(mut source: mpsc::Receiver<Id>, bridges: Arc<Bridges>, connection: Connection) -> Result<Never, CleanerError> {
        let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").map_err(|e| CleanerError::EmitterError(e))?;

        loop {
            let bridge = source.recv().await.ok_or(CleanerError::StreamError)?;
            let mut bridges = bridges.lock().await;

            let entry = match bridges.entry(bridge) {
                Entry::Vacant(_) => continue,
                Entry::Occupied(entry) => entry
            };

            if let Err(error) = entry.remove().1.cancel().await {
                warn!("Bridge with handle {} failed with error: {:?}", bridge, error);
            }
            
            emitter.bridge_broken(bridge).await?;
            info!("Cleaned handle {} from escape or error", bridge);
        }
    }
    async fn device_disconnection_bridge_cleaner(source: impl Stream<Item=AdapterEvent>, bridges: Arc<Bridges>, connection: Connection) -> Result<Never, CleanerError> {
        pin_mut!(source);

        let emitter = SignalEmitter::new(&connection, "/us/colbystuff/Bluekey").map_err(|e| CleanerError::EmitterError(e))?;
        while let Some(event) = source.next().await {
            if let AdapterEvent::DeviceRemoved(address) = event {
                let mut bridges = bridges.lock().await;
                for (handle, (_, bridge)) in bridges.extract_if(|_, (_, bridge)| bridge.target() == address) {
                    if let Err(error) = bridge.cancel().await {
                        warn!("Bridge with handle {} failed with error: {:?}", handle, error);
                    }

                    emitter.bridge_broken(handle).await?;
                }                
            }
        };
        
        Err(CleanerError::StreamError)
    }

    async fn run(adapter: Arc<Adapter>, keyboard: Arc<Keyboard>, mouse: Arc<Mouse>) -> Result<BluekeyError, zbus::Error> {
        let bridges = Arc::new(Mutex::new(HashMap::new()));
        let (sender, reciever) = mpsc::channel(16);
        
        let connection = {
            let escape_shortcut = SharedShortcut::new(Arc::new(Vec::from([KeyCode(125), KeyCode(1)])));
            let bridges = Self {
                bridge_id_source: IdSource::new(),
                bridges: bridges.clone(),
                bridge_broken_sender: sender,

                escape_shortcut: escape_shortcut.clone(),

                keyboard_server: keyboard.clone(),
                mouse_server: mouse.clone(),
            };

            let config = Config {
                keyboard_escape_shortcut: escape_shortcut
            };

            zbus::connection::Builder::session()?
                .name("us.colbystuff.Bluekey")?
                .serve_at("/us/colbystuff/Bluekey", bridges)?
                .serve_at("/us/colbystuff/Bluekey", config)?
                .build()
                .await?
        };
        
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        let source = dbus.receive_name_owner_changed().await?;

        
        Ok(select! {
            error = Self::disconnection_bridge_cleaner(source, bridges.clone(), connection.clone()) => unwrap(error).into(),
            error = Self::death_bridge_cleaner(reciever, bridges.clone(), connection.clone()) => unwrap(error).into(),
            error = Self::device_disconnection_bridge_cleaner(adapter.clone().events().await.unwrap(), bridges.clone(), connection.clone()) => unwrap(error).into(),
            error = devices::devices_tracker(connection.clone(), adapter.clone(), keyboard.clone(), mouse.clone()) => unwrap(error).into(),
        })
        


    }
}

#[interface(name = "us.colbystuff.Bluekey.Bridge1")]
impl Bluekey {
    /// Createa a new bridge from a mouse device to a bluetooth client 
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
        self.bridges.lock().await.insert(id, (name.into(), Bridge::Mouse(bridge, mac)));
        
        
        info!("Started mouse bridge from {} to {} with handle {}", mouse.display(), mac, id);
        Ok(id)
    }
    /// Create a new keyboard from a keybaord device to a bluetooth client
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
            Shortcut::new(&self.escape_shortcut),
            async move || {target.send(id).await.unwrap()}
        );        
        
        // Store the bridge
        self.bridges.lock().await.insert(id, (name.into(), Bridge::Keyboard(bridge, mac)));

        info!("Started keyboard bridge from {} to {} with handle {}", keyboard.display(), mac, id);
        Ok(id)
    }

    /// Destroy a device-bluetooth bridge and stop forwarding events. A client can only break a brige they created.
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


    /// Signifies that a device with a given handle was destroyed
    #[zbus(signal)]
    async fn bridge_broken(emitter: &SignalEmitter<'_>, bridge: Id) -> Result<(), zbus::Error>;

}

struct Config {
    keyboard_escape_shortcut: SharedShortcut
}
#[interface(name = "us.colbystuff.Bluekey.Configuration1")]
impl Config {
    #[zbus(property)]
    fn keyboard_escape_shortcut(&self) -> Vec<u16> {
        self.keyboard_escape_shortcut.keys().iter().map(|code| code.0).collect()
    }

    #[zbus(property)]
    fn set_keyboard_escape_shortcut(&self, value: Vec<u16>) {
        let value = value.into_iter().map(|value| KeyCode::new(value)).collect();
        self.keyboard_escape_shortcut.update(Arc::new(value));
    }
}



#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), zbus::Error> {
    env_logger::init();

    let session = bluer::Session::new().await.unwrap();
    let adapter = Arc::new(session.default_adapter().await.unwrap());

    let keyboard = Arc::new(Keyboard::new(adapter.clone()));
    let mouse = Arc::new(Mouse::new(adapter.clone()));
    let error = Bluekey::run(adapter.clone(), keyboard, mouse).await?;
    
    error!("Bluekey error, exiting: {}", error);
    
    
    Ok(())
}