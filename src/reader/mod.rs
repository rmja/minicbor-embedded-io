use embedded_io::asynch::Read;
use minicbor::{
    decode::{self, ArrayHeader, MapHeader},
    Decode, Decoder,
};

const BREAK: u8 = 0xFF;

mod asynch;
pub mod direct;

#[derive(Debug)]
pub enum Error {
    UnexpectedEof,
    BufferTooSmall,
    Io(embedded_io::ErrorKind),
    Decode(decode::Error),
}

impl<T: embedded_io::Error> From<T> for Error {
    fn from(value: T) -> Self {
        Error::Io(value.kind())
    }
}

pub trait CborArrayReader<C> {
    fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_array_item<R: CborReader>(
        &mut self,
        reader: &mut R,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub trait CborMapReader<C> {
    fn read_begin_map(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_map_item<R: CborReader>(
        &mut self,
        reader: &mut R,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub trait CborReader {
    async fn array<AR: CborArrayReader<()>>(
        &mut self,
        array_reader: &mut AR,
    ) -> Result<usize, Error>
    where
        Self: Sized,
    {
        self.array_with(array_reader, &mut ()).await
    }

    async fn array_with<C, AR: CborArrayReader<C>>(
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

    async fn map<MR: CborMapReader<()>>(&mut self, map_reader: &mut MR) -> Result<usize, Error>
    where
        Self: Sized,
    {
        self.map_with(map_reader, &mut ()).await
    }

    async fn map_with<C, MR: CborMapReader<C>>(
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
    async fn read<T>(&mut self) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, ()>,
    {
        self.read_with(&mut ()).await
    }

    /// Like [`CborReader::read`] but accepting a user provided decoding context.
    async fn read_with<C, T>(&mut self, ctx: &mut C) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>;

    /// Peek the next byte
    async fn peek(&mut self) -> Result<Option<u8>, Error>;
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

    async fn read_array_item<R: CborReader>(
        &mut self,
        reader: &mut R,
        _ctx: &mut (),
    ) -> Result<(), Error> {
        if let Some(item) = reader.read::<T>().await? {
            self.push(item);
        }

        Ok(())
    }
}
