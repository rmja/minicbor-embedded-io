use embedded_io::asynch::Read;
use minicbor::{Decode, Decoder};

use super::{CborReader, Error};

#[derive(Debug)]
pub struct AsyncCborReader<'b, R>
where
    R: Read,
{
    source: R,
    buf: &'b mut [u8],
    read: usize,
    decoded: usize,
}

impl<'b, R: Read> AsyncCborReader<'b, R> {
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
}

impl<R> AsyncCborReader<'_, R>
where
    R: Read,
{
    /// Like [`CborReader::read`] but accepting a user provided decoding context.
    pub async fn read_with<C, T>(&mut self, ctx: &mut C) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>,
    {
        loop {
            if self.decoded == 0 {
                if self.read_to_buf().await? == 0 {
                    return Ok(None);
                }
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
        if self.decoded == 0 {
            if self.read_to_buf().await? == 0 {
                return Ok(None);
            }
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

impl<R> CborReader for AsyncCborReader<'_, R>
where
    R: Read,
{
    async fn read_with<C, T>(&mut self, ctx: &mut C) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>,
    {
        self.read_with(ctx).await
    }

    async fn peek(&mut self) -> Result<Option<u8>, Error> {
        self.peek().await
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
        let mut reader = AsyncCborReader::new(cbor.as_slice(), &mut buf);
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
        let mut reader = AsyncCborReader::new(cbor.as_slice(), &mut buf);

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

        async fn read_array_item<R: CborReader>(
            &mut self,
            reader: &mut R,
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
        let mut reader = AsyncCborReader::new(cbor.as_slice(), &mut buf);

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
        let mut reader = AsyncCborReader::new(cbor.as_slice(), &mut buf);

        let mut array_reader = TestArrayReader;
        let mut ctx = Vec::new();
        reader
            .array_with(&mut array_reader, &mut ctx)
            .await
            .unwrap();

        assert_eq!(&[1, 2, 3], ctx.as_slice());
    }
}
