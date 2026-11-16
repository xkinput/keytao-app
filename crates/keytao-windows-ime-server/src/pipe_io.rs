use std::io;

pub fn classify_read(result: Result<(), i32>, bytes_read: u32) -> io::Result<Option<usize>> {
    match result {
        Ok(()) | Err(234) => Ok(Some(bytes_read as usize)), // ERROR_MORE_DATA
        Err(232) => Ok(None),                               // ERROR_NO_DATA
        Err(109 | 233) => Ok(Some(0)), // ERROR_BROKEN_PIPE / ERROR_PIPE_NOT_CONNECTED
        Err(code) => Err(io::Error::from_raw_os_error(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_read;

    #[test]
    fn successful_reads_return_the_byte_count() {
        assert_eq!(classify_read(Ok(()), 7).unwrap(), Some(7));
        assert_eq!(classify_read(Ok(()), 0).unwrap(), Some(0));
    }

    #[test]
    fn no_data_retries_instead_of_reporting_eof() {
        assert_eq!(classify_read(Err(232), 0).unwrap(), None);
    }

    #[test]
    fn disconnected_pipes_report_eof() {
        for code in [109, 233] {
            assert_eq!(classify_read(Err(code), 0).unwrap(), Some(0));
        }
    }

    #[test]
    fn partial_messages_preserve_the_bytes_read() {
        assert_eq!(classify_read(Err(234), 4).unwrap(), Some(4));
    }

    #[test]
    fn other_errors_preserve_the_raw_code() {
        assert_eq!(
            classify_read(Err(5), 0).unwrap_err().raw_os_error(),
            Some(5)
        );
    }
}
