use embedded_io::asynch::Read;
use minicbor::{decode, Decode, Decoder};

#[derive(Debug)]
pub struct CborReader<'b, R>
where
    R: Read,
{
    reader: R,
    buf: &'b mut [u8],
    read: usize,
    decoded: usize,
}

pub enum Error {
    UnexpectedEof,
    Io(embedded_io::ErrorKind),
    Decode(decode::Error),
}

impl<T: embedded_io::Error> From<T> for Error {
    fn from(value: T) -> Self {
        Error::Io(value.kind())
    }
}

impl<'b, R: Read> CborReader<'b, R> {
    /// Create a new reader
    ///
    /// The provided `buf` must be sufficiently large to contain what corresponds
    /// to one decode item.
    pub fn new(reader: R, buf: &'b mut [u8]) -> Self {
        Self {
            reader,
            buf,
            read: 0,
            decoded: 0,
        }
    }
}

impl<R> CborReader<'_, R>
where
    R: Read,
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
                let len = self.reader.read(&mut self.buf[self.read..]).await?;
                if len == 0 {
                    return if self.read == 0 {
                        Ok(None)
                    } else {
                        Err(Error::UnexpectedEof)
                    };
                }

                self.read += len;
            }

            let decoded: Option<T> =
                Self::try_decode_with(&self.buf[self.decoded..self.read], ctx)?;
            if decoded.is_some() {
                return Ok(decoded);
            }

            // Remove the just decoded value from the buffer by moving the
            // remaining, unused bytes in the buffer to the beginning
            self.buf.copy_within(self.decoded..self.read, 0);
            self.read -= self.decoded;
            self.decoded = 0;
        }
    }

    fn try_decode_with<'a, C, T: Decode<'a, C>>(
        buf: &'a [u8],
        ctx: &mut C,
    ) -> Result<Option<T>, Error> {
        let mut decoder = Decoder::new(buf);

        match decoder.decode_with(ctx) {
            Ok(decoded) => Ok(Some(decoded)),
            Err(e) if e.is_end_of_input() => Ok(None),
            Err(e) => Err(Error::Decode(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn can_read() {}
}
