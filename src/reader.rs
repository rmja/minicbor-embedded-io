use core::marker::PhantomData;

use embedded_io_async::Read;
use minicbor::{data::Type, decode, Decode, Decoder};

const BREAK: u8 = 0xFF;

#[derive(Debug)]
pub enum Error {
    UnexpectedEof,
    BufferTooSmall,
    Io(embedded_io_async::ErrorKind),
    Decode(decode::Error),
    #[cfg(feature = "alloc")]
    TryReserveError,
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

impl<T: embedded_io_async::Error> From<T> for Error {
    fn from(value: T) -> Self {
        Error::Io(value.kind())
    }
}

pub trait CborArrayReader<C> {
    fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut C) -> Result<(), Error>;

    async fn read_array_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        ctx: &mut C,
    ) -> Result<(), Error>;
}

pub trait CborMapReader<C> {
    fn read_begin_map(&mut self, len: Option<u64>, ctx: &mut C) -> Result<(), Error>;

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

#[derive(Debug)]
struct ArrayHeader(pub Option<u64>);

impl<'b, C> Decode<'b, C> for ArrayHeader {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, decode::Error> {
        let pos = d.position();
        let ty = d.datatype()?;
        match ty {
            Type::Array => {}
            Type::ArrayIndef => {}
            ty => {
                return Err(decode::Error::type_mismatch(ty)
                    .with_message("expected array")
                    .at(pos))
            }
        }

        let buf = d.input();
        let available = &buf[pos..];
        if available.is_empty() {
            return Err(decode::Error::end_of_input());
        }

        let head = decode::info::Size::head(available[0])?;
        if available.len() < head {
            return Err(decode::Error::end_of_input());
        }

        // Advance the decoder to the beginning of the array items
        d.set_position(pos + head);

        match decode::info::Size::tail(&available[..head])? {
            decode::info::Size::Head => Ok(Self(Some(0))),
            decode::info::Size::Bytes(_) => Err(decode::Error::type_mismatch(ty)),
            decode::info::Size::Items(len) => Ok(Self(Some(len))),
            decode::info::Size::Indef => Ok(Self(None)),
        }
    }
}

#[derive(Debug)]
struct MapHeader(pub Option<u64>);

impl<'b, C> Decode<'b, C> for MapHeader {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, decode::Error> {
        let pos = d.position();
        let ty = d.datatype()?;
        match ty {
            Type::Map => {}
            Type::MapIndef => {}
            ty => {
                return Err(decode::Error::type_mismatch(ty)
                    .with_message("expected map")
                    .at(pos))
            }
        }

        let buf = d.input();
        let available = &buf[pos..];
        if available.is_empty() {
            return Err(decode::Error::end_of_input());
        }

        let head = decode::info::Size::head(available[0])?;
        if available.len() < head {
            return Err(decode::Error::end_of_input());
        }

        // Advance the decoder to the beginning of the array items
        d.set_position(pos + head);

        match decode::info::Size::tail(&available[..head])? {
            decode::info::Size::Head => Ok(Self(Some(0))),
            decode::info::Size::Bytes(_) => Err(decode::Error::type_mismatch(ty)),
            decode::info::Size::Items(len) => Ok(Self(Some(len))),
            decode::info::Size::Indef => Ok(Self(None)),
        }
    }
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
            array_reader.read_begin_array(len, ctx)?;
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
            map_reader.read_begin_map(len, ctx)?;
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
            let bytes = &self.buf[self.decoded..self.read];

            #[cfg(feature = "defmt")]
            defmt::trace!("Decoder item bytes: {:02x}", bytes);

            let mut decoder = Decoder::new(bytes);
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
    fn read_begin_array(&mut self, len: Option<u64>, _ctx: &mut ()) -> Result<(), Error> {
        if let Some(len) = len {
            self.try_reserve_exact(len as usize)
                .map_err(|_| Error::TryReserveError)?;
        }

        Ok(())
    }

    async fn read_array_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        _ctx: &mut (),
    ) -> Result<(), Error> {
        if let Some(item) = reader.read::<T>().await? {
            self.try_reserve(1).map_err(|_| Error::TryReserveError)?;
            self.push(item);
        }

        Ok(())
    }
}

pub struct MapEntryReader<T: for<'b> MapEntryDecode<'b>> {
    entry_decode: PhantomData<T>,
}

impl<T: for<'b> MapEntryDecode<'b>> MapEntryReader<T> {
    pub fn new() -> Self {
        Self {
            entry_decode: PhantomData,
        }
    }
}

impl<T: for<'b> MapEntryDecode<'b>> CborMapReader<T> for MapEntryReader<T> {
    fn read_begin_map(&mut self, _len: Option<u64>, _ctx: &mut T) -> Result<(), Error> {
        Ok(())
    }

    async fn read_map_item<'b, R: Read>(
        &mut self,
        reader: &mut CborReader<'b, R>,
        ctx: &mut T,
    ) -> Result<(), Error> {
        reader.read_with::<T, Self>(ctx).await?;
        Ok(())
    }
}

impl<'d, T: for<'b> MapEntryDecode<'b>> Decode<'d, T> for MapEntryReader<T> {
    fn decode(d: &mut Decoder<'d>, ctx: &mut T) -> Result<Self, decode::Error> {
        T::decode_entry(ctx, d)?;
        Ok(Self::new())
    }
}

pub trait MapEntryDecode<'b> {
    fn decode_entry(&mut self, d: &mut Decoder<'b>) -> Result<(), decode::Error>;
}

#[cfg(test)]
mod tests {
    use core::iter::repeat;

    use embassy_sync::{
        blocking_mutex::raw::CriticalSectionRawMutex,
        pipe::{Pipe, Writer},
    };

    use crate::reader::CborArrayReader;

    use super::*;

    #[test]
    fn can_decode_small_array_header() {
        // Given
        let cbor: [u8; 5] = [0xf4, 0x83, 0x01, 0x02, 0x03];
        let mut d = Decoder::new(&cbor);
        d.set_position(1); // Skip first byte in buffer

        // When
        let header = ArrayHeader::decode(&mut d, &mut ()).unwrap();

        // Then
        assert_eq!(Some(3), header.0);
        assert_eq!(2, d.position());
    }

    #[test]
    fn can_decode_large_array_header() {
        // Given
        let cbor: [u8; 28] = [
            0xf4, 0x98, 0x18, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
            0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x18,
        ];
        let mut d = Decoder::new(&cbor);
        d.set_position(1); // Skip first byte in buffer

        // When
        let header = ArrayHeader::decode(&mut d, &mut ()).unwrap();

        // Then
        assert_eq!(Some(24), header.0);
        assert_eq!(3, d.position());
    }

    #[tokio::test]
    async fn can_read_small_array_manually() {
        let mut buf = [0; 16];
        let cbor: [u8; 5] = [0xf4, 0x83, 0x01, 0x02, 0x03];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);
        assert_eq!(false, reader.read::<bool>().await.unwrap().unwrap()); // Something before the array
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

    #[tokio::test]
    async fn can_read_large_array_manually() {
        let mut buf = [0; 16];
        let cbor: [u8; 28] = [
            0xf4, 0x98, 0x18, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
            0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x18,
        ];
        let mut reader = CborReader::new(cbor.as_slice(), &mut buf);
        assert_eq!(false, reader.read::<bool>().await.unwrap().unwrap()); // Something before the array
        assert_eq!(
            24,
            reader
                .read::<ArrayHeader>()
                .await
                .unwrap()
                .unwrap()
                .0
                .unwrap()
        );

        for i in 1..=24 {
            assert_eq!(i, reader.read::<u8>().await.unwrap().unwrap());
        }
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
        fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut Vec<u8>) -> Result<(), Error> {
            if let Some(len) = len {
                ctx.reserve_exact(len as usize);
            }

            Ok(())
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

    #[cfg(feature = "alloc")]
    #[tokio::test]
    async fn can_read_fixed_array_fuzz() {
        can_read_fixed_array_fuzz_case(1).await;
        can_read_fixed_array_fuzz_case(2).await;
        can_read_fixed_array_fuzz_case(3).await;
        can_read_fixed_array_fuzz_case(4).await;
        can_read_fixed_array_fuzz_case(5).await;
        can_read_fixed_array_fuzz_case(6).await;
        can_read_fixed_array_fuzz_case(7).await;
        can_read_fixed_array_fuzz_case(8).await;
        can_read_fixed_array_fuzz_case(9).await;
        can_read_fixed_array_fuzz_case(10).await;
    }

    #[cfg(feature = "alloc")]
    async fn can_read_fixed_array_fuzz_case(chunk_size: usize) {
        use embedded_io_async::Write;

        // Given
        const ITEM: &str = "wmbus-XXXXXXXXXXXXXXXX";
        const LEN: usize = 950;
        let strings: Vec<&str> = repeat(ITEM).take(LEN).collect();
        let cbor = minicbor::to_vec(strings.as_slice()).unwrap();

        static mut PIPE: Pipe<CriticalSectionRawMutex, 20> = Pipe::new();
        let pipe = unsafe { &mut PIPE };
        let (reader, writer) = pipe.split();

        // When
        let deserialize = body_reader(reader);
        let write = tokio::task::spawn(ingest(writer, cbor, chunk_size));

        let (deserialized, _) = tokio::join!(deserialize, write);

        // Then
        assert_eq!(LEN, deserialized.len());
        for item in deserialized {
            assert_eq!(ITEM, &item.0);
        }

        async fn body_reader(reader: impl Read) -> Vec<ArrayItem> {
            let mut cbor_item_buf = [0; 1 + 22]; // text(22) "wmbus-XXXXXXXXXXXXXXXX"
            let mut reader = CborReader::new(reader, &mut cbor_item_buf);
            let mut entries = Vec::new();
            reader.array(&mut entries).await.unwrap();
            entries
        }

        async fn ingest(
            mut writer: Writer<'_, CriticalSectionRawMutex, 20>,
            cbor: Vec<u8>,
            chunk_size: usize,
        ) {
            for chunk in cbor.chunks(chunk_size) {
                writer.write_all(chunk).await.unwrap();
            }
        }

        struct ArrayItem(String);

        impl<'b> Decode<'b, ()> for ArrayItem {
            fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
                let text = d.str()?;
                assert_eq!(ITEM, text);
                Ok(ArrayItem(text.to_string()))
            }
        }
    }
}
