use crate::error::DecodeError;

pub type Result<T> = std::result::Result<T, DecodeError>;

pub fn take<'p>(input: &mut &'p [u8], n: usize) -> Result<&'p [u8]> {
    if input.len() < n {
        return Err(DecodeError::UnexpectedEof);
    }

    let (head, tail) = input.split_at(n);
    *input = tail;

    Ok(head)
}
