use embedded_io::asynch::Read;
use minicbor::{
    decode::{self, ArrayHeader, MapHeader},
    Decode, Decoder,
};

const BREAK: u8 = 0xFF;

#[derive(Debug)]
pub enum Error {
    UnexpectedEof,
    BufferTooSmall,
    Io(embedded_io::ErrorKind),
    Decode(decode::Error),
}

#[cfg(feature = "defmt")]
impl defmt::Format for Error {
    fn format(&self, f: defmt::Formatter) {
        match self {
            Error::Decode(_) => defmt::write!(f, "Decode"),
            error => defmt::Format::format(error, f),
        }
    }
}

impl<T: embedded_io::Error> From<T> for Error {
    fn from(value: T) -> Self {
        Error::Io(value.kind())
    }
}

pub trait CborArrayReader<C> {
    fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_array_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub trait CborMapReader<C> {
    fn read_begin_map(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_map_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct CborReader<'b, R>
where
    R: Read,
{
    source: R,
    buf: &'b mut [u8],
    read: usize,
    decoded: usize,
}

impl<'b, R: Read> CborReader<'b, R> {
    /// Create a new reader
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one decode item.
    pub fn new(source: R, buf: &'b mut [u8]) -> Self {
        Self {
            source,
            buf,
            read: 0,
            decoded: 0,
        }
    }

    /// Read an array using a [`CborArrayReader`].
    pub async fn array<AR: CborArrayReader<()>>(
        &mut self,
        array_reader: &mut AR,
    ) -> Result<usize, Error>
    where
        Self: Sized,
    {
        self.array_with(array_reader, &mut ()).await
    }

    /// Read an array using a [`CborArrayReader`] accepting a user provided decoding context.
    pub async fn array_with<C, AR: CborArrayReader<C>>(
        &mut self,
        array_reader: &mut AR,
        ctx: &mut C,
    ) -> Result<usize, Error>
    where
        Self: Sized,
    {
        let mut count = 0;
        if let Some(header) = self.read::<ArrayHeader>().await? {
            let len = header.0;
            array_reader.read_begin_array(len, ctx);
            if let Some(len) = len {
                for _ in 0..len {
                    array_reader.read_array_item(self, ctx).await?;
                }
                count = len as usize;
            } else {
                while self.peek().await?.ok_or(Error::UnexpectedEof)? != BREAK {
                    array_reader.read_array_item(self, ctx).await?;
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Read a map using a [`CborMapReader`].
    pub async fn map<MR: CborMapReader<()>>(&mut self, map_reader: &mut MR) -> Result<usize, Error>
    where
        Self: Sized,
    {
        self.map_with(map_reader, &mut ()).await
    }

    /// Read a map using a [`CborMapReader`] accepting a user provided decoding context.
    pub async fn map_with<C, MR: CborMapReader<C>>(
        &mut self,
        map_reader: &mut MR,
        ctx: &mut C,
    ) -> Result<usize, Error>
    where
        Self: Sized,
    {
        let mut count = 0;
        if let Some(header) = self.read::<MapHeader>().await? {
            let len = header.0;
            map_reader.read_begin_map(len, ctx);
            if let Some(len) = len {
                for _ in 0..len {
                    map_reader.read_map_item(self, ctx).await?;
                }
                count = len as usize;
            } else {
                while self.peek().await?.ok_or(Error::UnexpectedEof)? != BREAK {
                    map_reader.read_map_item(self, ctx).await?;
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Read the next CBOR value and decode it
    pub async fn read<T>(&mut self) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, ()>,
    {
        self.read_with(&mut ()).await
    }

    /// Like [`CborReader::read`] but accepting a user provided decoding context.
    pub async fn read_with<C, T>(&mut self, ctx: &mut C) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>,
    {
        loop {
            if self.decoded == 0 && self.read_to_buf().await? == 0 {
                return Ok(None);
            }

            // Read an item from the buffer
            let mut decoder = Decoder::new(&self.buf[self.decoded..self.read]);
            let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
            if decoded.is_some() {
                self.decoded += decoder.position();
                return Ok(decoded);
            } else if self.decoded == 0 && self.read == self.buf.len() {
                return Err(Error::BufferTooSmall);
            }

            // Remove the decoded values from the buffer by moving the
            // remaining, unused bytes in the buffer to the beginning
            self.buf.copy_within(self.decoded..self.read, 0);
            self.read -= self.decoded;
            self.decoded = 0;
        }
    }

    /// Peek the next byte in the buffer
    async fn peek(&mut self) -> Result<Option<u8>, Error> {
        if self.decoded == 0 && self.read_to_buf().await? == 0 {
            return Ok(None);
        }

        Ok(Some(self.buf[self.decoded]))
    }

    async fn read_to_buf(&mut self) -> Result<usize, Error> {
        let len = self.source.read(&mut self.buf[self.read..]).await?;
        if len == 0 {
            return if self.read == 0 {
                Ok(0)
            } else {
                Err(Error::UnexpectedEof)
            };
        }

        self.read += len;
        Ok(len)
    }

    /// Try and decode an item from the decoder.
    /// Ignore end-of-input error as that, for now, signifies that we need to read more bytes
    /// from the underlying reader.
    fn try_decode_with<'a, C, T: Decode<'a, C>>(
        decoder: &mut Decoder<'a>,
        ctx: &mut C,
    ) -> Result<Option<T>, Error> {
        match decoder.decode_with(ctx) {
            Ok(decoded) => Ok(Some(decoded)),
            Err(e) if e.is_end_of_input() => Ok(None),
            Err(e) => Err(Error::Decode(e)),
        }
    }
}

#[cfg(feature = "alloc")]
impl<T, A: core::alloc::Allocator> CborArrayReader<()> for alloc::vec::Vec<T, A>
where
    for<'b> T: Decode<'b, ()>,
{
    fn read_begin_array(&mut self, len: Option<u64>, _ctx: &mut ()) {
        if let Some(len) = len {
            self.reserve_exact(len as usize);
        }
    }

    async fn read_array_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        _ctx: &mut (),
    ) -> Result<(), Error> {
        if let Some(item) = reader.read::<T>().await? {
            self.push(item);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use minicbor::decode::ArrayHeader;

    use crate::reader::CborArrayReader;

    use super::*;

    #[tokio::test]
    async fn can_read_manually() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);
        assert_eq!(
            3,
            reader
                .read::<ArrayHeader>()
                .await
                .unwrap()
                .unwrap()
                .0
                .unwrap()
        );

        assert_eq!(1, reader.read::<u8>().await.unwrap().unwrap());
        assert_eq!(2, reader.read::<u8>().await.unwrap().unwrap());
        assert_eq!(3, reader.read::<u8>().await.unwrap().unwrap());
        assert!(reader.read::<u8>().await.unwrap().is_none());
    }

    #[cfg(feature = "alloc")]
    #[tokio::test]
    async fn can_read_with_vec() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);

        let mut vec = Vec::new();
        reader.array(&mut vec).await.unwrap();

        assert_eq!(&[1, 2, 3], vec.as_slice());
    }

    struct TestArrayReader;

    impl CborArrayReader<Vec<u8>> for TestArrayReader {
        fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut Vec<u8>) {
            if let Some(len) = len {
                ctx.reserve_exact(len as usize);
            }
        }

        async fn read_array_item<'b, R: Read>(
            &mut self,
            reader: &mut CborReader<'b, R>,
            ctx: &mut Vec<u8>,
        ) -> Result<(), Error> {
            if let Some(item) = reader.read::<u8>().await? {
                ctx.push(item);
            }

            Ok(())
        }
    }

    #[tokio::test]
    async fn can_read_fixed_array() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);

        let mut array_reader = TestArrayReader;
        let mut ctx = Vec::new();
        reader
            .array_with(&mut array_reader, &mut ctx)
            .await
            .unwrap();

        assert_eq!(&[1, 2, 3], ctx.as_slice());
    }

    #[tokio::test]
    async fn can_read_inf_array() {
        let mut buf = [0; 16];
        let cbor: [u8; 5] = [0x9F, 0x01, 0x02, 0x03, 0xFF];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);

        let mut array_reader = TestArrayReader;
        let mut ctx = Vec::new();
        reader
            .array_with(&mut array_reader, &mut ctx)
            .await
            .unwrap();

        assert_eq!(&[1, 2, 3], ctx.as_slice());
    }
}
