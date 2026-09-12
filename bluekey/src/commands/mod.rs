use std::fmt::Display;

use thiserror::Error;

pub mod bluekey;
pub mod bridge;
pub mod list;
pub mod shortcut;

#[derive(Error, Debug)]
pub enum Error<'a> {
    #[error("DBus interface error.\n{0}")]
    BusMethod(zbus::fdo::Error),
    #[error("{0}")]
    BusConnection(BusConnectionError),
    #[error("{0}")]
    Address(AddressError<'a>),
    #[error("Bluekey bridge was destroyed.\nCaused by: {0}")]
    BridgeDestroyed(&'static str),
    #[error("Lost Bluekey event stream.")]
    LostBluekey,
    #[error("{0}")]
    Bluer(BluerError<'a>),
    #[error("{0}")]
    ShortcutFormatting(ShortcutFormattingError<'a>),
}
impl<'a> From<zbus::fdo::Error> for Error<'a> {
    fn from(value: zbus::fdo::Error) -> Self {
        Error::BusMethod(value)
    }
}
impl<'a> From<zbus::Error> for Error<'a> {
    fn from(value: zbus::Error) -> Self {
        Error::BusMethod(value.into())
    }
}

#[derive(Error, Debug, Clone)]
pub enum BusConnectionError {
    System(Option<zbus::Error>),
    Session(Option<zbus::Error>),
    Both(Option<zbus::Error>, Option<zbus::Error>),
}

impl Display for BusConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System(None) => write!(f, "\"us.colbystuff.Bluekey\" is not available on the system bus."),
            Self::Session(None) => write!(f, "\"us.colbystuff.Bluekey\" is not available on session bus."),
            Self::System(Some(error)) => write!(f, "Error connecting to system bus: {error}."),
            Self::Session(Some(error)) => write!(f, "Error connecting to sessioin bus: {error}"),

            Self::Both(system, session) => {
                writeln!(f, "Error connecting to DBus interace:")?;
                match system {
                    None => writeln!(f, "\"us.colbystuff.Bluekey\" is not available on the system bus."),
                    Some(error) => write!(f, "Error connecting to system bus: {error}.")
                }?;
                match session {
                    None => write!(f, "\"us.colbystuff.Bluekey\" is not available on session bus."),
                    Some(error) => write!(f, "Error connecting to session bus: {error}.")
                }
            }
        }
    }
}
impl<'a> From<BusConnectionError> for Error<'a> {
    fn from(value: BusConnectionError) -> Self {
        Error::BusConnection(value)
    }
}

#[derive(Error, Debug)]
pub enum AddressError<'a> {
    #[error("Invalid MAC address formatting for \"{0}\"")]
    AddressFormatting(&'a str),
    #[error("No dvices with alias \"{0}\"")]
    NoSuchAlias(&'a str),
    #[error("Multiple devices with alias \"{0}\"")]
    MultipleAliases(&'a str),
    
}

impl<'a> From<AddressError<'a>> for Error<'a> {
    fn from(value: AddressError<'a>) -> Self {
        Error::Address(value)
    }
}

#[derive(Error, Debug)]
pub enum BluerError<'a> {
    #[error("BlueR error finding alias \"{0}\": {1}")]
    Alias(&'a str, bluer::Error),
    #[error("Error connecting to BlueR: {0}")]
    Connection(bluer::Error),
    #[error("Error accessing BlueR: {0}")]
    Method(bluer::Error)
}
impl<'a> From<BluerError<'a>> for Error<'a> {
    fn from(value: BluerError<'a>) -> Self {
        Error::Bluer(value)
    }
} 
trait ToBluerError<T> {
    fn alias_error<'a>(self, alias: &'a str) -> Result<T, BluerError<'a>>;
    fn connecting_error<'a>(self) -> Result<T, BluerError<'a>>;
    fn method_error<'a>(self) -> Result<T, BluerError<'a>>;
}
impl<T> ToBluerError<T> for Result<T, bluer::Error> {
    fn alias_error<'a>(self, alias: &'a str) -> Result<T, BluerError<'a>> {
        self.map_err(|e| BluerError::Alias(alias, e))
    }
    fn connecting_error<'a>(self) -> Result<T, BluerError<'a>> {
        self.map_err(|e| BluerError::Connection(e))
    }
    fn method_error<'a>(self) -> Result<T, BluerError<'a>> {
        self.map_err(|e| BluerError::Method(e))
    }
}


#[derive(Error, Debug)]
pub enum ShortcutFormattingError<'a> {
    #[error("Invalid character \"{0}\" in shortcut.")]
    InvalidCharacter(char),
    #[error("Unknown key \"{0}\" in shortcut.")]
    InvalidKey(&'a str)
}
impl<'a> From<ShortcutFormattingError<'a>> for Error<'a> {
    fn from(value: ShortcutFormattingError<'a>) -> Self {
        Error::ShortcutFormatting(value)
    }
}