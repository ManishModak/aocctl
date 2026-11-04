//! [`Transport`] over Linux i2c-dev (`/dev/i2c-N`), addressed to the monitor's
//! DDC/CI slave 0x37. Holds an exclusive `flock` on the device for its lifetime,
//! so concurrent aocctl calls (and ddcutil, which locks the same way) don't
//! interleave packets.

use crate::ddc::{DdcError, Transport};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const I2C_SLAVE: libc::c_ulong = 0x0703;
const DDC_ADDR: libc::c_ulong = 0x37;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct I2cDev {
    file: File,
}

impl I2cDev {
    pub fn open(path: &Path) -> Result<Self, DdcError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| DdcError::Access(format!("{}: {e}", path.display())))?;
        let fd = file.as_raw_fd();
        let start = Instant::now();
        // SAFETY: fd is a valid open descriptor owned by `file`.
        while unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            if start.elapsed() > LOCK_TIMEOUT {
                return Err(DdcError::Access(format!(
                    "{} is busy (locked by another program)",
                    path.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: I2C_SLAVE takes the 7-bit address as an integer argument.
        if unsafe { libc::ioctl(fd, I2C_SLAVE, DDC_ADDR) } != 0 {
            let e = std::io::Error::last_os_error();
            return Err(DdcError::Access(format!("{}: set address: {e}", path.display())));
        }
        Ok(I2cDev { file })
    }
}

impl Transport for I2cDev {
    fn write(&mut self, bytes: &[u8]) -> Result<(), DdcError> {
        self.file.write_all(bytes).map_err(|e| DdcError::Io(e.to_string()))
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<(), DdcError> {
        self.file.read_exact(buf).map_err(|e| DdcError::Io(e.to_string()))
    }
}
