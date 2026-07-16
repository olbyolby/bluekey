// A very awful, but technically functional, Bluetooth keyboard emulator(and an unused battery emulator)
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::sleep,
};

mod hid;
mod keyboard;


#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    //main2().await;

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let board = keyboard::start_keyboard(adapter).await?;

    println!("Press enter to quit");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    
    loop {
        tokio::select! {
            _ = lines.next_line() => break,
            _ = sleep(Duration::from_millis(25)) => {
                println!("A pressed?");
                let _ = board.press(0x04).await;
                sleep(Duration::from_millis(10)).await;
                let _ = board.release(0x04).await;
            }
        }
    }

    Ok(())

    
}
