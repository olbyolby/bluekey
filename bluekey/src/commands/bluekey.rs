use std::{fmt::Display, path::Path};

use zbus::{Connection, names::BusName, proxy};
use clap::Args;

use crate::ev_key_map;

#[proxy(
    interface="us.colbystuff.Bluekey.Bridge1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
pub trait Bridge {
    async fn bridge_mouse(&self, mouse: &Path, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn bridge_keyboard(&self, keyboard: &Path, mac: &str) -> Result<u64, zbus::fdo::Error>;
    async fn destroy_bridge(&self, handle: u64) -> Result<(), zbus::fdo::Error>;

    #[zbus(signal)]
    fn bridge_broken(&self, id: u64) -> zbus::Result<()>;
}
#[proxy(
    interface="us.colbystuff.Bluekey.Configuration1",
    default_service="us.colbystuff.Bluekey",
    default_path="/us/colbystuff/Bluekey"
)]
pub trait Config {
    #[zbus(property)]
    fn keyboard_escape_shortcut(&self) -> Result<Shortcut, zbus::fdo::Error>;

    #[zbus(property)]
    fn set_keyboard_escape_shortcut(&self, value: Shortcut) -> Result<(), zbus::fdo::Error>;
}
#[proxy(
    interface="us.colbystuff.Bluekey.Device1",
    default_service="us.colbystuff.Bluekey"
)]
pub trait Device {
    #[zbus(property)]
    fn address(&self) -> Result<String, zbus::fdo::Error>;

    #[zbus(property)]
    fn has_keyboard(&self) -> Result<bool, zbus::fdo::Error>;

    #[zbus(property)]
    fn has_mouse(&self) -> Result<bool, zbus::fdo::Error>;

    #[zbus(property)]
    fn power(&self) -> Result<u8, zbus::fdo::Error>;
}


// Representation of a keyboard shortcut, collection of evdev event IDs
pub struct Shortcut {
    keys: Vec<u16>
}
impl Shortcut {
    #[allow(dead_code)]
    pub fn new(keys: Vec<u16>) -> Self {
        Self {
            keys
        }
    }
}
impl TryFrom<zvariant::OwnedValue> for Shortcut {
    type Error = zvariant::Error;
    fn try_from(value: zvariant::OwnedValue) -> Result<Self, Self::Error> {
        Ok(Self { keys: Vec::<u16>::try_from(value)?})
    }
}
impl<'a> Into<zvariant::Value<'a>> for Shortcut {
    fn into(self) -> zvariant::Value<'a> {
        self.keys.into()
    }
}
impl Display for Shortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys = self.keys.iter();
        if let Some(key) = keys.next() {
            match ev_key_map::evdev_keycode_to_name(*key) {
                Some(name) => write!(f, "{}", name),
                None => write!(f, "<keycode: {}>", key)
            }?
        }

        for key in keys {
            match ev_key_map::evdev_keycode_to_name(*key) {
                Some(name) => write!(f, "+{}", name),
                None => write!(f, "+<keycode: {}>", key)
            }?
        };
        Ok(())
    }
}

// Type of connection bus chosen by user
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionBus {
    Session,
    System,
    Unspecified
}
// Argument for selecting system or session bus
#[derive(Args)]
#[group(required = false, multiple = false)]
pub struct ConnectionBusArgument {
    #[arg(long)]
    /// Use the system's bus
    system: bool,
    #[arg(long)]
    /// Use the user's bus
    user: bool
}
impl ConnectionBusArgument {
    pub fn choice(&self) -> ConnectionBus {
        if self.user {
            ConnectionBus::Session
        } else if self.system {
            ConnectionBus::System
        } else {
            ConnectionBus::Unspecified
        }
    }

    // Connect 
    pub async fn connect(&self) -> Result<Connection, super::Error<'_>> {
        use super::BusConnectionError;
        use ConnectionBus::*;
        
        match self.choice() {
            System => match Self::verify_connection(Connection::system()).await? {
                Err(error) => Err(BusConnectionError::System(error).into()),
                Ok(connection) => Ok(connection)
            },
            Session => match Self::verify_connection(Connection::session()).await? {
                Err(error) => Err(BusConnectionError::Session(error).into()),
                Ok(connection) => Ok(connection)
            },
            Unspecified => match Self::verify_connection(Connection::session()).await? {
                Ok(connection) => Ok(connection),
                Err(error) => match Self::verify_connection(Connection::system()).await? {
                    Err(e) => Err(BusConnectionError::Both(error, e).into()),
                    Ok(connection) => Ok(connection)
                }
            }
        }
    }

    async fn verify_connection(connection: impl Future<Output = Result<Connection, zbus::Error>>) -> Result<Result<Connection, Option<zbus::Error>>, super::Error<'static>> {
        match connection.await {
            Err(error) => Ok(Err(Some(error))),
            Ok(connection) => match has_bluekey(&connection).await {
                Ok(true) => Ok(Ok(connection)),
                Ok(false) => Ok(Err(None)),
                Err(e) => Err(e.into())
            }
        }
    }
}

async fn has_bluekey(connection: &Connection) -> Result<bool, super::Error<'static>> {
    let proxy = zbus::fdo::DBusProxy::new(&connection).await?;

    Ok(proxy.name_has_owner(BusName::from_static_str("us.colbystuff.Bluekey").unwrap()).await?)
}