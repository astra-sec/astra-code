use crate::model::{ApiProtocol, Harness, Job};
use std::io::{Read, Write};
use std::str::FromStr;

const MAGIC: &[u8] = b"ASTRA_CODE_JOB_V1\0";
const MAX_FIELD_SIZE: usize = 64 * 1024 * 1024;

pub fn write_job(mut writer: impl Write, job: &Job) -> Result<(), String> {
    writer
        .write_all(MAGIC)
        .map_err(|e| format!("write job header: {e}"))?;
    for field in [
        job.harness.as_str(),
        job.api.as_str(),
        &job.base_url,
        &job.model,
        &job.token,
        &job.prompt,
    ] {
        write_field(&mut writer, field.as_bytes())?;
    }
    writer.flush().map_err(|e| format!("flush job: {e}"))
}

pub fn read_job(mut reader: impl Read) -> Result<Job, String> {
    let mut magic = vec![0_u8; MAGIC.len()];
    reader
        .read_exact(&mut magic)
        .map_err(|e| format!("read job header: {e}"))?;
    if magic != MAGIC {
        return Err("invalid job protocol header".to_owned());
    }

    let harness = Harness::from_str(&read_string(&mut reader)?)?;
    let api = ApiProtocol::from_str(&read_string(&mut reader)?)?;
    let base_url = read_string(&mut reader)?;
    let model = read_string(&mut reader)?;
    let token = read_string(&mut reader)?;
    let prompt = read_string(&mut reader)?;
    Ok(Job {
        harness,
        api,
        base_url,
        model,
        token,
        prompt,
    })
}

fn write_field(writer: &mut impl Write, value: &[u8]) -> Result<(), String> {
    if value.len() > MAX_FIELD_SIZE || value.len() > u32::MAX as usize {
        return Err(format!("job field is too large: {} bytes", value.len()));
    }
    writer
        .write_all(&(value.len() as u32).to_be_bytes())
        .and_then(|_| writer.write_all(value))
        .map_err(|e| format!("write job field: {e}"))
}

fn read_string(reader: &mut impl Read) -> Result<String, String> {
    let mut length = [0_u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|e| format!("read job field length: {e}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FIELD_SIZE {
        return Err(format!("job field exceeds {MAX_FIELD_SIZE} bytes"));
    }
    let mut value = vec![0_u8; length];
    reader
        .read_exact(&mut value)
        .map_err(|e| format!("read job field: {e}"))?;
    String::from_utf8(value).map_err(|_| "job field is not valid UTF-8".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{read_job, write_job};
    use crate::model::{ApiProtocol, Harness, Job};

    #[test]
    fn job_round_trip_preserves_multiline_text() {
        let expected = Job {
            harness: Harness::Pi,
            api: ApiProtocol::OpenAiChatCompletions,
            base_url: "http://host.docker.internal:8080/v1".to_owned(),
            model: "test/model".to_owned(),
            token: "secret\0with newline\n".to_owned(),
            prompt: "修复这个项目。\nDo not stop early.".to_owned(),
        };
        let mut encoded = Vec::new();
        write_job(&mut encoded, &expected).unwrap();
        assert_eq!(read_job(encoded.as_slice()).unwrap(), expected);
    }

    #[test]
    fn rejects_wrong_header() {
        assert!(read_job(b"not-a-job".as_slice()).is_err());
    }
}
