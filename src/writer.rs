use embedded_io_async::Write;
use minicbor::{
    encode::{self, write::EndOfSlice},
    Encode,
};

pub enum Error {
    Io(embedded_io_async::ErrorKind),
    Encode(encode::Error<EndOfSlice>),
}

impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(io) => f.debug_tuple("Io").field(io).finish(),
            Self::Encode(enc) => f.debug_tuple("Encode").field(enc).finish(),
        }
    }
}

impl<T: embedded_io_async::Error> From<T> for Error {
    fn from(value: T) -> Self {
        Error::Io(value.kind())
    }
}

pub trait CborArrayWriter<C> {
    fn write_begin_array(&mut self, len: Option<u64>, ctx: &mut C);
    async fn write_array_item<'b, R: Write>(
        &mut self,
        writer: &mut CborWriter<'b, R>,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub trait CborMapWriter<C> {
    fn write_begin_map(&mut self, len: Option<u64>, ctx: &mut C);
    async fn write_map_item<'b, R: Write>(
        &mut self,
        writer: &mut CborWriter<'b, R>,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub struct CborWriter<'b, W>
where
    W: Write,
{
    sink: W,
    buf: &'b mut [u8],
}

impl<'b, W: Write> CborWriter<'b, W> {
    /// Create a new writer
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one encoded item.
    pub fn new(sink: W, buf: &'b mut [u8]) -> Self {
        Self { sink, buf }
    }

    /// Encode and write a CBOR value and return its size in bytes.
    pub async fn write<T: Encode<()>>(&mut self, val: T) -> Result<usize, Error> {
        self.write_with(val, &mut ()).await
    }

    /// Like [`AsyncWriter::write`] but accepting a user provided encoding context.
    pub async fn write_with<C, T: Encode<C>>(
        &mut self,
        value: T,
        ctx: &mut C,
    ) -> Result<usize, Error> {
        let mut cursor = Cursor(&mut self.buf, 0);
        minicbor::encode_with(value, &mut cursor, ctx).map_err(Error::Encode)?;

        let len = cursor.1;
        self.sink.write_all(&self.buf[..len]).await?;
        Ok(len)
    }

    pub async fn flush(&mut self) -> Result<(), Error> {
        self.sink.flush().await?;
        Ok(())
    }
}

pub struct Cursor<'a, W>(&'a mut W, usize);

impl minicbor::encode::Write for Cursor<'_, &mut [u8]> {
    type Error = EndOfSlice;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        let slice = &mut self.0[self.1..];
        let len = buf.len();
        slice[..len].copy_from_slice(buf);
        self.1 += len;
        Ok(())
    }
}
