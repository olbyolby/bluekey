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
    
    let mut count = 0;
    let mut sending = false;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                if let Ok(Some(text)) = line {
                    if text == "send" {
                        sending = true;
                    } else if text == "don't" {
                        sending = false;
                    } else if text == "once" {
                        let _ = board.press(0x04).await;
                        sleep(Duration::from_millis(100)).await;
                        let _ = board.release(0x04).await;
                    }
                } else {
                    break
                }
            },
            _ = sleep(Duration::from_millis(250)) => {
                if sending {
                    println!("A pressed?");
                    if count % 5 == 0 {
                        println!("Shift pressed?");
                        let _ = board.press(0xE1).await;
                    } 

                    let _ = board.press(0x04).await;
                    sleep(Duration::from_millis(100)).await;
                    let _ = board.release(0x04).await;
                    if count % 5 == 0 {
                        let _ = board.release(0xE1).await;
                    }

                    count += 1;
                }
            }
        }
    }

    Ok(())

    
}
