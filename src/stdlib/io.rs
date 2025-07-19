use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use crate::lib::arc::Arc;

pub type IoResult<T> = Result<T, IoError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoError {
    WouldBlock,
    InvalidInput,
    UnexpectedEof,
    Other,
}

pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize>;
    
    fn read_exact(&mut self, mut buf: &mut [u8]) -> IoResult<()> {
        while !buf.is_empty() {
            match self.read(buf) {
                Ok(0) => break,
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                }
                Err(e) => return Err(e),
            }
        }
        if !buf.is_empty() {
            Err(IoError::UnexpectedEof)
        } else {
            Ok(())
        }
    }
}

pub trait Write {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize>;
    
    fn flush(&mut self) -> IoResult<()>;
    
    fn write_all(&mut self, mut buf: &[u8]) -> IoResult<()> {
        while !buf.is_empty() {
            match self.write(buf) {
                Ok(0) => return Err(IoError::Other),
                Ok(n) => buf = &buf[n..],
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

pub struct StdinBuffer {
    buffer: Vec<u8>,
    read_pos: usize,
    echo_enabled: bool,
}

impl StdinBuffer {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            read_pos: 0,
            echo_enabled: true, // Default to echo enabled
        }
    }
    
    pub fn new_with_echo(echo_enabled: bool) -> Self {
        Self {
            buffer: Vec::new(),
            read_pos: 0,
            echo_enabled,
        }
    }
    
    pub fn set_echo(&mut self, enabled: bool) {
        self.echo_enabled = enabled;
    }
    
    pub fn echo_enabled(&self) -> bool {
        self.echo_enabled
    }
    
    pub fn push_char(&mut self, ch: char) {
        crate::debug_trace!("StdinBuffer::push_char called with '{}', echo_enabled: {}", ch, self.echo_enabled);
        
        // Echo the character to display if echo is enabled
        if self.echo_enabled {
            crate::debug_trace!("Echoing character: '{}'", ch);
            crate::print!("{}", ch);
        }
        
        let mut buf = [0; 4];
        let bytes = ch.encode_utf8(&mut buf).as_bytes();
        self.buffer.extend_from_slice(bytes);
    }
    
    pub fn push_char_no_echo(&mut self, ch: char) {
        crate::debug_trace!("StdinBuffer::push_char_no_echo called with '{}'", ch);
        
        let mut buf = [0; 4];
        let bytes = ch.encode_utf8(&mut buf).as_bytes();
        self.buffer.extend_from_slice(bytes);
    }
    
    pub fn push_byte(&mut self, byte: u8) {
        self.buffer.push(byte);
    }
    
    pub fn available(&self) -> usize {
        self.buffer.len() - self.read_pos
    }
    
    fn compact(&mut self) {
        if self.read_pos > 0 {
            self.buffer.drain(..self.read_pos);
            self.read_pos = 0;
        }
    }
}

impl Read for StdinBuffer {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let available = self.available();
        if available == 0 {
            return Err(IoError::WouldBlock);
        }
        
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.buffer[self.read_pos..self.read_pos + to_read]);
        self.read_pos += to_read;
        
        // Compact buffer if we've read more than half
        if self.read_pos > self.buffer.len() / 2 {
            self.compact();
        }
        
        Ok(to_read)
    }
}

pub struct Stdin {
    buffer: Arc<Mutex<StdinBuffer>>,
}

impl Stdin {
    pub fn new(buffer: Arc<Mutex<StdinBuffer>>) -> Self {
        Self { buffer }
    }
    
    /// Enable or disable character echoing for this stdin
    pub fn set_echo(&mut self, enabled: bool) {
        self.buffer.lock().set_echo(enabled);
    }
    
    /// Check if echo is currently enabled
    pub fn echo_enabled(&self) -> bool {
        self.buffer.lock().echo_enabled()
    }
    
    pub fn read_line(&mut self) -> IoResult<String> {
        use crate::stdlib::waker::{Waker, register_stdin_waker, unregister_stdin_waker};
        
        let mut line = String::new();
        let mut byte = [0u8; 1];
        let mut waker = Waker::new();
        
        // Register for stdin events
        unsafe {
            register_stdin_waker(&mut waker as *mut Waker);
        }
        
        let result = loop {
            match self.read(&mut byte) {
                Ok(0) => break Err(IoError::UnexpectedEof),
                Ok(_) => {
                    let ch = byte[0] as char;
                    if ch == '\n' {
                        break Ok(line);
                    }
                    line.push(ch);
                }
                Err(IoError::WouldBlock) => {
                    // Process any pending keyboard input first
                    crate::drivers::keyboard::process_pending_input();
                    
                    // Check if we got woken up by keyboard input
                    if !waker.poll_and_reset() {
                        // Still no input, wait for interrupt
                        x86_64::instructions::hlt();
                    }
                    // Loop back to try reading again
                }
                Err(e) => break Err(e),
            }
        };
        
        // Unregister the waker before returning
        unregister_stdin_waker(&mut waker as *mut Waker);
        
        result
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.buffer.lock().read(buf)
    }
}

pub struct Stdout;

impl Stdout {
    pub const fn new() -> Self {
        Self
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        for &byte in buf {
            // Handle UTF-8 properly in the future
            crate::print!("{}", byte as char);
        }
        Ok(buf.len())
    }
    
    fn flush(&mut self) -> IoResult<()> {
        // Display is unbuffered, so nothing to do
        Ok(())
    }
}

pub struct IoHandles {
    pub stdin: Stdin,
    pub stdout: Stdout,
}

impl IoHandles {
    pub fn new(stdin_buffer: Arc<Mutex<StdinBuffer>>) -> Self {
        Self {
            stdin: Stdin::new(stdin_buffer),
            stdout: Stdout::new(),
        }
    }
    
    pub fn new_with_echo(echo_enabled: bool) -> (Self, Arc<Mutex<StdinBuffer>>) {
        let stdin_buffer = Arc::new(Mutex::new(StdinBuffer::new_with_echo(echo_enabled)));
        let io_handles = Self {
            stdin: Stdin::new(stdin_buffer.clone()),
            stdout: Stdout::new(),
        };
        (io_handles, stdin_buffer)
    }
}