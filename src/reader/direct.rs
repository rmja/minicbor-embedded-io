use embedded_io::asynch::{DirectRead, DirectReadHandle};
use minicbor::{decode::ArrayHeader, Decode, Decoder};

use super::Error;

pub struct DirectCborReader<'b, 'm, R>
where
    R: DirectRead,
{
    buf: &'b mut [u8],
    buf_written: usize,
    buf_decoded: usize,
    reader: R,
    current: Option<Handle<'m, R>>,
}

pub struct Handle<'m, R>
where
    R: DirectRead,
{
    pos: usize,
    handle: R::Handle<'m>,
}

impl<'r, 'm, R> DirectCborReader<'r, 'm, R>
where
    R: DirectRead,
{
    /// Create a new reader
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one decode item.
    pub fn new(buf: &'r mut [u8], source: R) -> Self {
        Self {
            buf,
            buf_written: 0,
            buf_decoded: 0,
            reader: source,
            current: None,
        }
    }
}

impl<'m, R> DirectCborReader<'_, 'm, R>
where
    R: DirectRead + 'm,
{
    /// Peek the next byte in the buffer
    async fn peek(&mut self) -> Result<Option<u8>, Error> {
        if self.buf_decoded == 0 {
            if self.read_to_buf().await? == 0 {
                return Ok(None);
            }
        }

        if self.buf_decoded < self.buf_written {
            Ok(Some(self.buf[self.buf_decoded]))
        } else {
            let current = self.current.as_mut().unwrap();
            let slice = current.as_slice();
            if current.pos < slice.len() {
                Ok(Some(slice[current.pos]))
            } else {
                Err(Error::UnexpectedEof)
            }
        }
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
            if self.buf_decoded == 0 {
                if self.read_to_buf().await? == 0 {
                    return Ok(None);
                }
            }

            // First, try and read an item from the buffer
            if self.buf_written > 0 {
                let mut decoder = Decoder::new(&self.buf[self.buf_decoded..self.buf_written]);
                let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
                if decoded.is_some() {
                    self.buf_decoded += decoder.position();
                    return Ok(decoded);
                }
            }

            let current = self.current.as_mut().unwrap();
            let handle_slice = current.as_slice();
            if !handle_slice.is_empty() {
                // Second, try and read from between buffer and active handle
                if self.buf_written > 0 {
                    // "borrow" from the current active handle so that we try to read a single item
                    // that spans between the buffer and the handle
                    let unused = self.buf.len() - self.buf_written;
                    let to_copy = usize::min(unused, handle_slice.len());

                    let pos = self.buf_written;
                    self.buf[pos..pos + to_copy].copy_from_slice(&handle_slice[..to_copy]);

                    let mut decoder = Decoder::new(&self.buf[..self.buf_written]);
                    let decoded =
                        Self::try_decode_with(&mut decoder, ctx)?.ok_or(Error::BufferTooSmall)?;

                    current.pos = decoder.position() - self.buf_decoded;
                    self.buf_written = 0;

                    return Ok(Some(decoded));
                }

                // Third, read from the active handle
                let mut decoder = Decoder::new(&handle_slice[current.pos..]);
                let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
                if decoded.is_some() {
                    current.pos += decoder.position();
                    return Ok(decoded);
                }

                // Save the unused bytes from the active handle into the buffer
                let to_copy = handle_slice.len() - current.pos;
                if to_copy > self.buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                self.buf[..to_copy].copy_from_slice(&handle_slice[current.pos..]);
                self.buf_written += to_copy;
            }

            self.buf_written -= self.buf_decoded;
            self.buf_decoded = 0;
        }
    }

    async fn read_to_buf(&mut self) -> Result<usize, Error> {
        if let Some(handle) = self.current.as_ref() {
            if handle.handle.is_completed() {
                return if self.buf_written == 0 {
                    Ok(0)
                } else {
                    Err(Error::UnexpectedEof)
                };
            }
        }

        let mut reads = 0;
        loop {
            // The lifetime for &mut self is limited to call to read_with(),
            // but we need a reader that exceeds this lifetime to store
            // a "current" reference inside the reader, as the active
            // slice returned from the reader must live between calls to read_with().
            //
            // # SAFETY
            //
            // We only advance the the reader behind a &mut self
            let reader = &mut self.reader;
            let reader = unsafe { core::mem::transmute::<&mut R, &'m mut R>(reader) };
            let handle = reader.read().await?;
            reads += 1;
            self.current = Some(Handle::new(handle));

            let handle = self.current.as_ref().unwrap();
            let slice = handle.as_slice();
            let len = slice.len();

            if self.buf_written + len <= self.buf.len() {
                // This handle can fit entirely in the buffer
                let offset = self.buf_written;
                self.buf[offset..offset + len].copy_from_slice(slice);
                self.buf_written += len;

                if handle.handle.is_completed() {
                    return Ok(reads);
                }
            } else {
                return Ok(reads);
            }
        }
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

impl<'m, R> Handle<'m, R>
where
    R: DirectRead + 'm,
{
    const fn new(handle: R::Handle<'m>) -> Self {
        Self { pos: 0, handle }
    }

    fn as_slice(&self) -> &[u8] {
        self.handle.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn can_read_manually() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let source = cbor.as_slice();
        let mut reader = DirectCborReader::new(&mut buf, source);
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

    // #[cfg(feature = "alloc")]
    // #[tokio::test]
    // async fn can_read_with_vec() {
    //     let mut buf = [0; 16];
    //     let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
    //     let mut reader = DirectCborReader::new(cbor.as_slice(), &mut buf);

    //     let mut vec = Vec::new();
    //     reader.array(&mut vec).await.unwrap();

    //     assert_eq!(&[1, 2, 3], vec.as_slice());
    // }
}
