use core::{
    marker::PhantomData,
};

use embedded_io::asynch::{DirectRead, DirectReadHandle};
use futures_intrusive::sync::LocalMutex;
use minicbor::{Decode, Decoder, decode::ArrayHeader};

use super::Error;

pub struct DirectCborReader<'b, R>
where
    R: DirectRead,
{
    inner: LocalMutex<Inner<'b>>,
    reader: PhantomData<R>,
}

struct Inner<'b> {
    buf: &'b mut [u8],
    read: usize,
    decoded: usize,
}

pub struct Handle<'m, R>
where
    R: DirectRead,
{
    source: LocalMutex<&'m mut R>,
    pos: usize,
    handle: Option<R::Handle<'m>>,
}

impl<'r, R> DirectCborReader<'r, R>
where
    R: DirectRead,
{
    /// Create a new reader
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one decode item.
    pub fn new(buf: &'r mut [u8]) -> Self {
        Self {
            inner: LocalMutex::new(
                Inner {
                    buf,
                    read: 0,
                    decoded: 0,
                },
                false,
            ),
            reader: PhantomData,
        }
    }
}

impl<R> DirectCborReader<'_, R>
where
    R: DirectRead,
{
    pub fn new_handle<'m>(&self, source: &'m mut R) -> Handle<'m, R> {
        Handle {
            source: LocalMutex::new(source, false),
            pos: 0,
            handle: None,
        }
    }

    /// Read the next CBOR value and decode it
    pub async fn read<'m, T>(&self, handle: &mut Handle<'m, R>) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, ()>,
    {
        self.read_with(handle, &mut ()).await
    }

    /// Like [`CborReader::read`] but accepting a user provided decoding context.
    pub async fn read_with<'m, C, T>(
        &self,
        handle: &mut Handle<'m, R>,
        ctx: &mut C,
    ) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>,
    {
        let mut inner = self.inner.try_lock().unwrap();

        loop {
            if inner.decoded == 0 {
                if let Some(handle) = handle.handle.as_ref() {
                    if handle.is_completed() {
                        return if inner.read == 0 {
                            Ok(None)
                        } else {
                            Err(Error::UnexpectedEof)
                        };
                    }
                }

                loop {
                    let handle = handle.advance_source().await?;
                    let slice = handle.as_slice();
                    let len = slice.len();

                    if inner.read + len <= inner.buf.len() {
                        // This handle can fit entirely in the buffer
                        let offset = inner.read;
                        inner.buf[offset..offset + len].copy_from_slice(slice);
                        inner.read += len;

                        if handle.is_completed() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }

            // First, try and read an item from the buffer
            let mut decoder = Decoder::new(&inner.buf[inner.decoded..inner.read]);
            let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
            if decoded.is_some() {
                inner.decoded += decoder.position();
                return Ok(decoded);
            }

            let handle_slice = handle.as_slice();
            if !handle_slice.is_empty() {
                // Second, try and read from between buffer and active handle
                if inner.read > 0 {
                    // "borrow" from the current active handle so that we try to read a single item
                    // that spans between the buffer and the handle
                    let unused = inner.buf.len() - inner.read;
                    let to_copy = usize::min(unused, handle_slice.len());

                    let pos = inner.read;
                    inner.buf[pos..pos + to_copy].copy_from_slice(&handle_slice[..to_copy]);

                    let mut decoder = Decoder::new(&inner.buf[..inner.read]);
                    let decoded =
                        Self::try_decode_with(&mut decoder, ctx)?.ok_or(Error::BufferTooSmall)?;

                    handle.pos = decoder.position() - inner.decoded;
                    inner.read = 0;

                    return Ok(Some(decoded));
                }

                // Third, read from the active handle
                let mut decoder = Decoder::new(&handle_slice[handle.pos..]);
                let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
                if decoded.is_some() {
                    handle.pos += decoder.position();
                    return Ok(decoded);
                }

                // Save the unused bytes from the active handle into the buffer
                let to_copy = handle_slice.len() - handle.pos;
                if to_copy > inner.buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                inner.buf[..to_copy].copy_from_slice(&handle_slice[handle.pos..]);
                inner.read += to_copy;
            }

            inner.read -= inner.decoded;
            inner.decoded = 0;
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
    R: DirectRead,
{
    async fn advance_source(&mut self) -> Result<&mut R::Handle<'m>, Error> {
        let mut source = self.source.try_lock().unwrap();
        let source = unsafe { core::mem::transmute::<&mut R, &mut R>(&mut source) };
        self.handle = Some(source.read().await?);
        Ok(self.handle.as_mut().unwrap())
    }

    fn as_slice(&self) -> &[u8] {
        self.handle.as_ref().unwrap().as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn can_read_manually() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let reader = DirectCborReader::new(&mut buf);
        let mut source = cbor.as_slice();
        let mut handle = reader.new_handle(&mut source);
        assert_eq!(
            3,
            reader
                .read::<ArrayHeader>(&mut handle)
                .await
                .unwrap()
                .unwrap()
                .0
                .unwrap()
        );

        assert_eq!(1, reader.read::<u8>(&mut handle).await.unwrap().unwrap());
        assert_eq!(2, reader.read::<u8>(&mut handle).await.unwrap().unwrap());
        assert_eq!(3, reader.read::<u8>(&mut handle).await.unwrap().unwrap());
        assert!(reader.read::<u8>(&mut handle).await.unwrap().is_none());
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
