use clap::Args;

use super::{
    bluekey::{ConnectionBusArgument, KeyboardProxy},
    device::RemoteDeviceArgs,
    Error,
};

#[derive(Args)]
pub struct Send {
    #[clap(flatten)]
    bus: ConnectionBusArgument,

    #[clap(flatten)]
    remote_device: RemoteDeviceArgs,

    /// Delay between key events in milliseconds
    #[arg(long, default_value_t = 10)]
    delay: u32,

    /// Append an Enter key after the text
    #[arg(long)]
    enter: bool,

    /// Text to send as keyboard input
    text: String,
}

impl Send {
    pub async fn execute(&self) -> Result<(), Error<'_>> {
        let connection = self.bus.connect().await?;
        let address = self.remote_device.parse_mac().await?;

        let keyboard = KeyboardProxy::new(&connection).await?;

        let mut text = self.text.clone();
        if self.enter {
            text.push('\n');
        }

        keyboard.send_text(&address.to_string(), &text, self.delay).await?;

        Ok(())
    }
}
