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

/// Takes exactly `N` bytes as an array. Length is pre-checked by `take`,
/// so the conversion is infallible; `UnexpectedEof` is a defensive fallback.
pub fn take_array<const N: usize>(input: &mut &[u8]) -> std::result::Result<[u8; N], DecodeError> {
    take(input, N)?.try_into().map_err(|_| DecodeError::UnexpectedEof)
}
