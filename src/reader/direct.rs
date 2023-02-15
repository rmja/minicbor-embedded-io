use embedded_io::asynch::{DirectRead, DirectReadHandle};
use minicbor::{decode::ArrayHeader, Decode, Decoder};

use super::Error;

pub struct DirectCborReader<'b, 'm, R>
where
    R: DirectRead,
{
    buf: &'b mut [u8],
    read: usize,
    decoded: usize,
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
            read: 0,
            decoded: 0,
            reader: source,
            current: None,
        }
    }
}

impl<'m, R> DirectCborReader<'_, 'm, R>
where
    R: DirectRead + 'm,
{
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
            if self.decoded == 0 {
                if let Some(handle) = self.current.as_ref() {
                    if handle.handle.is_completed() {
                        return if self.read == 0 {
                            Ok(None)
                        } else {
                            Err(Error::UnexpectedEof)
                        };
                    }
                }

                loop {
                    // The lifetime for &mut self is limited to call to read_with(),
                    // but we need a reader that exceeds this lifetime to store
                    // a "current" reference inside the reader, as the active
                    // slice returned from the reader must live between calls to read_with().
                    //
                    // # SAFETY
                    //
                    // We only advance the the reader here behind a &mut self
                    let reader = &mut self.reader;
                    let reader = unsafe { core::mem::transmute::<&mut R, &'m mut R>(reader) };
                    let handle = reader.read().await?;
                    self.current = Some(Handle::new(handle));

                    let handle = self.current.as_ref().unwrap();
                    let slice = handle.as_slice();
                    let len = slice.len();

                    if self.read + len <= self.buf.len() {
                        // This handle can fit entirely in the buffer
                        let offset = self.read;
                        self.buf[offset..offset + len].copy_from_slice(slice);
                        self.read += len;

                        if handle.handle.is_completed() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }

            let current = self.current.as_mut().unwrap();

            // First, try and read an item from the buffer
            let mut decoder = Decoder::new(&self.buf[self.decoded..self.read]);
            let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
            if decoded.is_some() {
                self.decoded += decoder.position();
                return Ok(decoded);
            }

            let handle_slice = current.as_slice();
            if !handle_slice.is_empty() {
                // Second, try and read from between buffer and active handle
                if self.read > 0 {
                    // "borrow" from the current active handle so that we try to read a single item
                    // that spans between the buffer and the handle
                    let unused = self.buf.len() - self.read;
                    let to_copy = usize::min(unused, handle_slice.len());

                    let pos = self.read;
                    self.buf[pos..pos + to_copy].copy_from_slice(&handle_slice[..to_copy]);

                    let mut decoder = Decoder::new(&self.buf[..self.read]);
                    let decoded =
                        Self::try_decode_with(&mut decoder, ctx)?.ok_or(Error::BufferTooSmall)?;

                    current.pos = decoder.position() - self.decoded;
                    self.read = 0;

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
                self.read += to_copy;
            }

            self.read -= self.decoded;
            self.decoded = 0;
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
