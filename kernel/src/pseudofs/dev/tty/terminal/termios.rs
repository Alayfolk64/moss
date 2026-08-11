#![allow(dead_code)]

use core::ops::{Deref, DerefMut};

use bytemuck::AnyBitPattern;
use linux_raw_sys::general::{
    B38400, CREAD, CS8, ECHO, ECHOCTL, ECHOE, ECHOK, ECHOKE, ICANON, ICRNL, IEXTEN, ISIG, IXON,
    ONLCR, OPOST, VDISCARD, VEOF, VEOL, VEOL2, VERASE, VINTR, VKILL, VLNEXT, VQUIT, VREPRINT,
    VSUSP, VWERASE, speed_t, tcflag_t,
};
use starry_signal::Signo;

#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
pub struct Termio {
    c_iflag: u16,
    c_oflag: u16,
    c_cflag: u16,
    c_lflag: u16,
    c_line: u8,
    c_cc: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
pub struct Termios {
    c_iflag: tcflag_t,
    c_oflag: tcflag_t,
    c_cflag: tcflag_t,
    c_lflag: tcflag_t,
    c_line: u8,
    c_cc: [u8; 19usize],
}

impl Default for Termios {
    fn default() -> Self {
        let mut result = Self {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: B38400 | CS8 | CREAD,
            c_lflag: ICANON | ECHO | ISIG | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
            c_line: 0,
            c_cc: [0; 19],
        };

        fn ctl(ch: u8) -> u8 {
            ch - 0x40
        }
        for (i, ch) in [
            (VINTR, ctl(b'C')),
            (VQUIT, ctl(b'\\')),
            (VSUSP, ctl(b'Z')),
            (VERASE, b'\x7f'),
            (VKILL, ctl(b'U')),
            (VEOF, ctl(b'D')),
            (VEOL, b'\0'),
            (VREPRINT, ctl(b'R')),
            (VDISCARD, ctl(b'O')),
            (VWERASE, ctl(b'W')),
            (VLNEXT, ctl(b'V')),
            (VEOL2, b'\0'),
        ] {
            result.c_cc[i as usize] = ch;
        }

        result
    }
}

impl Termios {
    pub fn from_termio(termio: Termio) -> Self {
        let mut termios = Self {
            c_iflag: termio.c_iflag as _,
            c_oflag: termio.c_oflag as _,
            c_cflag: termio.c_cflag as _,
            c_lflag: termio.c_lflag as _,
            c_line: termio.c_line,
            c_cc: [0; 19],
        };
        termios.c_cc[..termio.c_cc.len()].copy_from_slice(&termio.c_cc);
        termios
    }

    pub fn to_termio(self) -> Termio {
        let mut c_cc = [0; 8];
        c_cc.copy_from_slice(&self.c_cc[..8]);
        Termio {
            c_iflag: self.c_iflag as _,
            c_oflag: self.c_oflag as _,
            c_cflag: self.c_cflag as _,
            c_lflag: self.c_lflag as _,
            c_line: self.c_line,
            c_cc,
        }
    }

    pub fn special_char(&self, index: u32) -> u8 {
        self.c_cc[index as usize]
    }

    pub fn has_iflag(&self, flag: u32) -> bool {
        self.c_iflag & flag != 0
    }

    pub fn has_oflag(&self, flag: u32) -> bool {
        self.c_oflag & flag != 0
    }

    pub fn has_cflag(&self, flag: u32) -> bool {
        self.c_cflag & flag != 0
    }

    pub fn has_lflag(&self, flag: u32) -> bool {
        self.c_lflag & flag != 0
    }

    pub fn echo(&self) -> bool {
        self.has_lflag(ECHO)
    }

    pub fn canonical(&self) -> bool {
        self.has_lflag(ICANON)
    }

    pub fn contains_iexten(&self) -> bool {
        self.has_lflag(IEXTEN)
    }

    pub fn is_eol(&self, ch: u8) -> bool {
        if ch == b'\n' || ch == self.special_char(VEOL) {
            return true;
        }

        if self.contains_iexten() && ch == self.special_char(VEOL2) {
            return true;
        }

        false
    }

    pub fn signo_for(&self, ch: u8) -> Option<Signo> {
        // Linux uses zero as _POSIX_VDISABLE on this architecture.  Never
        // turn a disabled control slot into a signal-generating NUL byte.
        for (index, signo) in [
            (VINTR, Signo::SIGINT),
            (VQUIT, Signo::SIGQUIT),
            (VSUSP, Signo::SIGTSTP),
        ] {
            let special = self.special_char(index);
            if special != 0 && ch == special {
                return Some(signo);
            }
        }
        None
    }
}

#[repr(C)]
#[derive(Clone, Copy, AnyBitPattern)]
pub struct Termios2 {
    termios: Termios,
    c_ispeed: speed_t,
    c_ospeed: speed_t,
}

impl Default for Termios2 {
    fn default() -> Self {
        Self::new(Termios::default())
    }
}
impl Termios2 {
    pub fn new(termios: Termios) -> Self {
        Self {
            termios,
            c_ispeed: B38400,
            c_ospeed: B38400,
        }
    }
}

impl Deref for Termios2 {
    type Target = Termios;

    fn deref(&self) -> &Self::Target {
        &self.termios
    }
}

impl DerefMut for Termios2 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.termios
    }
}
