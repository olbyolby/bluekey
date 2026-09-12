use clap::Args;

use crate::{commands::bluekey::Shortcut, ev_key_map};

use super::{Error, bluekey::ConnectionBusArgument, ShortcutFormattingError};

#[derive(Args)]
pub struct EscapeShortcut {
    #[clap(flatten)]
    bus: ConnectionBusArgument,

    shortcut: Option<String>,
}

impl EscapeShortcut {
    pub async fn execute(&self) -> Result<(), Error<'_>>{
        let shortcut = self.shortcut.as_ref().map(|text| {
            let mut shortcut: Vec<u16> = Vec::new();
            for name in text.split("+") {
                let name = name.trim();
                if let Some(character) = name.chars().find(|c| !c.is_alphanumeric() && *c != '_') {
                    return Err(ShortcutFormattingError::InvalidCharacter(character))
                }
                
                shortcut.push(ev_key_map::name_to_evdev_keycode(name).ok_or(ShortcutFormattingError::InvalidKey(name))?);
            }
            
            Ok(Shortcut::new(shortcut))
        }).transpose()?;

        let connection = self.bus.connect().await?;
        

        let config = super::bluekey::ConfigProxy::new(&connection).await?;

        match shortcut {
            Some(shortcut) => {
                config.set_keyboard_escape_shortcut(shortcut).await?
            },
            None => {
                println!("Keyboard shortcut is: {}", config.keyboard_escape_shortcut().await?)
            }
        };

        Ok(())
    }
}