use embedded_io::asynch::{DirectRead, DirectReadHandle};
use minicbor::{decode::{ArrayHeader, MapHeader}, Decode, Decoder};

use super::Error;

pub struct DirectCborReader<'b, 'm, R>
where
    R: DirectRead,
{
    reader: R,
    buf: &'b mut [u8],
    buf_decoded: usize,
    read: usize,
    handle: Option<R::Handle<'m>>,
    handle_decoded: usize,
}

#[derive(Debug)]
pub struct Handle<'m, R>
where
    R: DirectRead,
{
    handle: R::Handle<'m>,
}

pub trait CborArrayReader<C> {
    fn read_begin_array(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_array_item<'b, 'm, R: DirectRead>(
        &mut self,
        reader: &'b mut DirectCborReader<'b, 'm, R>,
        ctx: &mut C,
    ) -> Result<(), Error> where 'b: 'm;
}

pub trait CborMapReader<C> {
    fn read_begin_map(&mut self, len: Option<u64>, ctx: &mut C);
    async fn read_map_item<'b, 'm, R: DirectRead>(
        &mut self,
        reader: &'b mut DirectCborReader<'b, 'm, R>,
        ctx: &mut C,
    ) -> Result<(), Error> where 'b: 'm;
}

impl<'b, R: DirectRead> DirectCborReader<'b, '_, R> {
    /// Create a new reader
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one decode item.
    pub fn new(reader: R, buf: &'b mut [u8]) -> Self {
        Self {
            reader,
            buf,
            buf_decoded: 0,
            read: 0,
            handle: None,
            handle_decoded: 0,
        }
    }
}

impl<'b, 'm, R> DirectCborReader<'b, 'm, R>
where
    R: DirectRead,
{
    pub async fn array<AR: CborArrayReader<()>>(
        &'m mut self,
        array_reader: &mut AR,
    ) -> Result<usize, Error> {
        self.array_with(array_reader, &mut ()).await
    }

    pub async fn array_with<C, AR: CborArrayReader<C>>(
        &'m mut self,
        array_reader: &mut AR,
        ctx: &mut C,
    ) -> Result<usize, Error> where 'b : 'm {
        todo!();
        // let mut count = 0;
        // if let Some(header) = self.read::<ArrayHeader>().await? {
        //     let len = header.0;
        //     array_reader.read_begin_array(len, ctx);
        //     if let Some(len) = len {
        //         for _ in 0..len {
        //             array_reader.read_array_item(self, ctx).await?;
        //         }
        //         count = len as usize;
        //     } else {
        //         todo!();
        //         // while self.peek().await?.ok_or(Error::UnexpectedEof)? != BREAK {
        //         //     array_reader.read_array_item(self, ctx).await?;
        //         //     count += 1;
        //         // }
        //     }
        // }

        // Ok(count)
    }

    pub async fn map<MR: CborMapReader<()>>(
        &'m mut self,
        map_reader: &mut MR,
    ) -> Result<usize, Error> {
        self.map_with(map_reader, &mut ()).await
    }

    pub async fn map_with<C, MR: CborMapReader<C>>(
        &'m mut self,
        map_reader: &mut MR,
        ctx: &mut C,
    ) -> Result<usize, Error> {
        todo!();
        // let mut count = 0;
        // if let Some(header) = self.read::<MapHeader>().await? {
        //     let len = header.0;
        //     map_reader.read_begin_map(len, ctx);
        //     if let Some(len) = len {
        //         for _ in 0..len {
        //             map_reader.read_map_item(self, ctx).await?;
        //         }
        //         count = len as usize;
        //     } else {
        //         todo!();
        //         // while self.peek().await?.ok_or(Error::UnexpectedEof)? != BREAK {
        //         //     map_reader.read_map_item(self, ctx).await?;
        //         //     count += 1;
        //         // }
        //     }
        // }

        // Ok(count)
    }

    /// Read the next CBOR value and decode it
    pub async fn read<T>(&'m mut self) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, ()>,
    {
        self.read_with(&mut ()).await
    }

    /// Like [`CborReader::read`] but accepting a user provided decoding context.
    pub async fn read_with<C, T>(&'m mut self, ctx: &mut C) -> Result<Option<T>, Error>
    where
        for<'a> T: Decode<'a, C>,
    {
        if self.buf_decoded == 0 {
            if let Some(handle) = self.handle.as_ref() {
                if handle.is_completed() {
                    return if self.read == 0 {
                        Ok(None)
                    } else {
                        Err(Error::UnexpectedEof)
                    };
                }
            }

            let (buffered, handle) =
                read_to_buf(&mut self.reader, &mut self.buf[self.read..]).await?;
            self.read += buffered;
            self.handle = Some(handle);

            // We have buffered as much as possible - the returned handle is either the last or could not fit in the buffer

            if self.read > 0 {
                // Copy from the handle to the buffer so that we can decode exactly one item
                let unused = self.buf.len() - self.read;
                let handle_slice = self.handle.as_ref().unwrap().as_slice();
                let to_copy = usize::min(unused, handle_slice.len());

                self.buf[self.read..self.read + to_copy].copy_from_slice(&handle_slice[..to_copy]);
                let mut decoder = Decoder::new(&self.buf[..self.read]);
                let decoded = Self::try_decode_with(&mut decoder, ctx)?;
                return Ok(decoded);
            }
        }

        let handle = self.handle.as_ref().unwrap();
        let handle_slice = handle.as_slice();

        // Decode directly from the handle slice
        let mut decoder = Decoder::new(&handle.as_slice()[self.buf_decoded..]);
        let decoded: Option<T> = Self::try_decode_with(&mut decoder, ctx)?;
        if decoded.is_some() {
            self.buf_decoded += decoder.position();
            return Ok(decoded);
        }

        // Copy the unused bytes from the handle to the buffer
        let carry = handle_slice.len() - self.buf_decoded;
        self.buf.copy_from_slice(&handle_slice[self.buf_decoded..]);
        self.read = carry;
        self.buf_decoded = 0;
        todo!();
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

async fn read_to_buf<'r, 'm, R: DirectRead>(
    reader: &'r mut R,
    buf: &mut [u8],
) -> Result<(usize, R::Handle<'m>), Error>
where
    'r: 'm,
{
    let mut pos = 0;
    // loop {
        let handle = reader.read().await?;
        let slice = handle.as_slice();
        let len = slice.len();

        if pos + len <= buf.len() {
            buf[pos..pos + len].copy_from_slice(slice);
            pos += len;
            return Err(Error::UnexpectedEof);
        } else {
            return Ok((pos, handle));
        }
    // }
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

    async fn read_array_item<'b, 'm, R: DirectRead>(
        &mut self,
        reader: &'b mut DirectCborReader<'b, 'm, R>,
        _ctx: &mut (),
    ) -> Result<(), Error> where 'b: 'm {
        if let Some(item) = reader.read::<T>().await? {
            self.push(item);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[cfg(feature = "alloc")]
    #[tokio::test]
    async fn can_read_with_vec() {
        let mut buf = [0; 16];
        let cbor: [u8; 4] = [0x83, 0x01, 0x02, 0x03];
        let mut reader = DirectCborReader::new(cbor.as_slice(), &mut buf);
    
        let mut vec = Vec::new();
        reader.array(&mut vec).await.unwrap();

        assert_eq!(&[1, 2, 3], vec.as_slice());
    }
}