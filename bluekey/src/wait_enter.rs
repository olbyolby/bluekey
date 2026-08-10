use std::os::fd::{AsRawFd, AsFd};
use tokio::io::unix::AsyncFd;

// Set the O_NONBLOCK flag on a file descriptor temporarily and safely
struct BlockingFlagWrapper<'a, T: AsFd> {
    fd: &'a mut T,
    flags: i32
}
impl<'a, T: AsFd> Drop for BlockingFlagWrapper<'a, T> {
    fn drop(&mut self) {
        unsafe {
            // Lord only knows what'll happen to the underlying 'T' if it's flaged are screwed up and can't be reverted, so panic
            // That violates the invariant that this will revert modified flags.
            if libc::fcntl(self.fd.as_fd().as_raw_fd(), libc::F_SETFL, self.flags) == -1 {
                Err(std::io::Error::last_os_error()).expect("Could not restore file descriptor flags.")
            }
        }
    }
}
impl<'a, T: AsFd> AsFd for BlockingFlagWrapper<'a, T> {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
impl<'a, T: AsFd> AsRawFd for BlockingFlagWrapper<'a, T> {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.as_fd().as_raw_fd()
    }
}
impl<'a, T: AsFd> BlockingFlagWrapper<'a, T> {
    fn new(fd: &'a mut T) -> std::io::Result<Self> {
        let flags = unsafe {
            // Get the current set of flags
            let flags = libc::fcntl(fd.as_fd().as_raw_fd(), libc::F_GETFL);
            if flags == -1 {
                return Err(std::io::Error::last_os_error())
            }

            // Set nonblock flag
            if libc::fcntl(fd.as_fd().as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
                return Err(std::io::Error::last_os_error())
            }

            flags
        };
        
        Ok(BlockingFlagWrapper { 
            fd,
            flags
        })
    }
}

// The remarkably complicated task of waiting for the user to press enter cancel-safely and without a thread keeping the program alive indefinitely, even after main() returns. 
// By default, tokio's stdin isn't actually async, and keeps a blocking thread alive. That thread will stay even after main() returns until the user presses enter. 
//   See: https://github.com/tokio-rs/tokio/issues/589
// So, the solution is to do actual sync input, but this requires setting the O_NONBLOCK flag for the global stdin file descriptor, which does terrible things to other, concurrent, IO operations.
//   See: https://github.com/rust-lang/rust/issues/100673
// As such, this function requires an StdinLock to prevent other threads screwing with it, and resets the O_NONBLOCK flag upon completion. 
pub async fn async_wait_enter<'a>(stdin: &mut std::io::StdinLock<'a>) -> std::io::Result<()> {
    let fd =  AsyncFd::new(BlockingFlagWrapper::new(stdin)?)?;
    
    // Wait for IO to be avalible
    loop {
        let mut ready = fd.readable().await?;

        // Read some input, check for enter, repeat until there is no more.
        loop {
            let result = unsafe {
                let mut buffer = [0u8; 16];
                match libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) {
                    -1 => Err(std::io::Error::last_os_error()),
                    0 => Ok(true),
                    length => Ok(buffer[..length as usize].contains(&('\n' as u8)))
                }
            };
            match result {
                Err(error) => match error.raw_os_error() {
                    #[allow(unreachable_patterns)] // EAGAIN == EWOULDBLOCK, currently, so Rust complains, but read's man states "a portable application should check for both possibilities"
                    Some(libc::EAGAIN | libc::EWOULDBLOCK) => ready.clear_ready(),
                    Some(libc::EINTR) => continue,
                    _ => return Err(error)
                },
                Ok(true) => return Ok(()),
                _ => continue
            };
        }
        
    }

}
