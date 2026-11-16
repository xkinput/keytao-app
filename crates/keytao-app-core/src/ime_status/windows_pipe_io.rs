use std::io;

pub(super) fn classify_read(result: Result<(), i32>, bytes_read: u32) -> io::Result<Option<usize>> {
    match result {
        Ok(()) => Ok(Some(bytes_read as usize)),
        // Win32 ERROR_NO_DATA means an empty PIPE_NOWAIT read, not EOF.
        Err(232) => Ok(None),
        // ERROR_BROKEN_PIPE / ERROR_PIPE_NOT_CONNECTED are real EOF.
        Err(109 | 233) => Ok(Some(0)),
        // ERROR_MORE_DATA preserves the partial message already read.
        Err(234) => Ok(Some(bytes_read as usize)),
        Err(code) => Err(io::Error::from_raw_os_error(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_read;

    #[test]
    fn no_data_retries_instead_of_reporting_eof() {
        assert_eq!(classify_read(Err(232), 0).unwrap(), None);
        assert_eq!(classify_read(Ok(()), 4).unwrap(), Some(4));
    }

    #[test]
    fn disconnected_pipes_report_eof() {
        for code in [109, 233] {
            assert_eq!(classify_read(Err(code), 0).unwrap(), Some(0));
        }
    }

    #[test]
    fn success_and_partial_messages_preserve_the_count() {
        for count in [0, 1, 32] {
            assert_eq!(classify_read(Ok(()), count).unwrap(), Some(count as usize));
            assert_eq!(
                classify_read(Err(234), count).unwrap(),
                Some(count as usize)
            );
        }
    }

    #[test]
    fn other_errors_preserve_the_raw_code() {
        assert_eq!(
            classify_read(Err(5), 0).unwrap_err().raw_os_error(),
            Some(5)
        );
    }
}
