use anyhow::Result;
use bincode::config::{BigEndian, Configuration, Fixint};
use bincode::Decode;
use tokio::io::{AsyncReadExt};
use tokio_serial::{DataBits, FlowControl, Parity, StopBits};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

//
// Standard serial communication settings for the PMSX003
//
const BAUD_RATE: u32 = 9600;
const DATA_BITS: DataBits = DataBits::Eight;
const FLOW_CONTROL: FlowControl = FlowControl::None;
const PARITY: Parity = Parity::None;
const STOP_BITS: StopBits = StopBits::One;

// Valid sensor frames begin with the sequence b"BM"
const START_OF_FRAME_0: u8 = b'B';
const START_OF_FRAME_1: u8 = b'M';
// ...followed by the frame size
const EXPECT_FRAME_SIZE: usize = size_of::<Frame>();
const FULL_FRAME_SIZE: usize = EXPECT_FRAME_SIZE + 4;

/// A PMSX003 data frame.
#[allow(dead_code)]
#[derive(Debug, Decode)]
pub struct Frame {
    /// PM1.0 concentration in standard units
    pub pm10_standard: u16,
    /// PM2.5 concentration in standard units
    pub pm25_standard: u16,
    /// PM10.0 concentration in standard units
    pub pm100_standard: u16,

    /// PM1.0 concentration in environmental units
    pub pm10_env: u16,
    /// PM2.5 concentration in environmental units
    pub pm25_env: u16,
    /// PM10.0 concentration in environmental units
    pub pm100_env: u16,

    /// Number of 0.3µm particles detected per 0.1L unit of air
    pub particles_03um: u16,
    /// Number of 0.5µm particles detected per 0.1L unit of air
    pub particles_05um: u16,
    /// Number of 1.0µm particles detected per 0.1L unit of air
    pub particles_10um: u16,
    /// Number of 2.5µm particles detected per 0.1L unit of air
    pub particles_25um: u16,
    /// Number of 5.0µm particles detected per 0.1L unit of air
    pub particles_50um: u16,
    /// Number of 10.0µm particles detected per 0.1L unit of air
    pub particles_100um: u16,

    _unused: u16,
    checksum: u16,
}

pub struct PMS {
    serial_port: SerialStream,
    decoder: Configuration<BigEndian, Fixint>,
}

impl PMS {
    /// Instantiate a low-level interface to the PMSX003 sensor
    pub fn new<PATH: Into<std::borrow::Cow<'static, str>>>(path: PATH) -> Result<Self> {
        let serial_port = tokio_serial::new(path, BAUD_RATE)
            .data_bits(DATA_BITS)
            .flow_control(FLOW_CONTROL)
            .parity(PARITY)
            .stop_bits(STOP_BITS)
            .open_native_async()?;
        let decoder = bincode::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding();

        Ok(PMS {
            serial_port,
            decoder,
        })
    }

    /// Poll the serial port for a valid PMSX003 frame.
    pub async fn get_frame(&mut self) -> Result<Frame> {
        // The portion of the checksum that is always the same. No need to sum
        // the initial four bytes
        const SUM_BASE: u16 = b'B' as u16
            + b'M' as u16
            + (EXPECT_FRAME_SIZE & 0xff) as u16
            + (EXPECT_FRAME_SIZE >> 8 & 0xff) as u16;

        // This is designed as a state engine due to the very primitive serial
        // interface (which doesn't provide buffering).
        #[derive(Debug)]
        enum ReadState {
            // Expecting b'B' (first start-of-frame byte)
            SOF1,
            // Expecting b'M' (second start-of-frame byte)
            SOF2,
            // Expecting frame size (fixed)
            Size,
            // Expecting frame data
            Data,
        }

        let mut framebuf = [0u8; FULL_FRAME_SIZE];
        let mut state = ReadState::SOF1;

        loop {
            let readbuf = match state {
                ReadState::SOF1 => &mut framebuf[0..=0],
                ReadState::SOF2 => &mut framebuf[1..=1],
                ReadState::Size => &mut framebuf[2..=3],
                ReadState::Data => &mut framebuf[4..],
            };

            // Block until enough bytes are ready
            self.serial_port.read_exact(readbuf).await?;

            match state {
                ReadState::SOF1 => {
                    // Expect b'B'
                    if readbuf[0] == START_OF_FRAME_0 {
                        state = ReadState::SOF2;
                        // Otherwise, keep looking for b'B'
                    }
                }
                ReadState::SOF2 => match readbuf[0] {
                    // Expect b'M'
                    START_OF_FRAME_1 => state = ReadState::Size,
                    // Previous byte false start-of-frame, and this is the actual?
                    START_OF_FRAME_0 => continue,
                    // Previous byte was false start-of-frame.
                    _ => state = ReadState::SOF1,
                },
                ReadState::Size => {
                    // readbuf is 2 bytes here, so the slice-to-array conversion
                    // can't fail
                    let frame_size = usize::from(u16::from_be_bytes(readbuf.try_into()?));
                    if frame_size == EXPECT_FRAME_SIZE {
                        state = ReadState::Data;
                    } else {
                        // Size was garbage. Just wait for another start-of-frame marker
                        state = ReadState::SOF1;
                    }
                }
                ReadState::Data => {
                    // Validate the checksum
                    let (frame, _): (Frame, usize) =
                        bincode::decode_from_slice(readbuf, self.decoder)?;
                    let expected: u16 =
                        SUM_BASE + readbuf.iter().take(26).copied().map(u16::from).sum::<u16>();
                    break if expected != frame.checksum {
                        Err(anyhow::anyhow!("checksum mismatch"))
                    } else {
                        Ok(frame)
                    };
                }
            }
        }
    }
}
