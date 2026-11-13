//! Borrow SQL results during JSON encoding; bound one complete line before emission.
use std::io::{self,Write};
use anyhow::{Context,Result};
use serde::{Serialize,Serializer,ser::{SerializeMap,SerializeSeq}};
use bradian_relational::{sql::SqlResultSet,typed_table::Value};

pub(crate) const DEFAULT_OUTPUT_BYTES:usize=16*1024*1024;
struct Cell<'a>(&'a Value);
impl Serialize for Cell<'_> {
    fn serialize<S:Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        let mut map=serializer.serialize_map(Some(2))?;
        // Retain the original JSON map's lexical field order, including time.
        match self.0 {
            Value::Null=>{ map.serialize_entry("type","null")?; map.serialize_entry("value",&Option::<bool>::None)?; }
            Value::Boolean(v)=>{ map.serialize_entry("type","boolean")?; map.serialize_entry("value",v)?; }
            Value::Int32(v)=>{ map.serialize_entry("type","int32")?; map.serialize_entry("value",v)?; }
            Value::Int64(v)=>{ map.serialize_entry("type","int64")?; map.serialize_entry("value",&v.to_string())?; }
            Value::Decimal(v)=>{ map.serialize_entry("type","decimal")?; map.serialize_entry("unscaled",&v.to_string())?; }
            Value::String(v)=>{ map.serialize_entry("type","text")?; map.serialize_entry("value",v)?; }
            Value::TimestampTz(v)=>{ map.serialize_entry("microseconds",&v.to_string())?; map.serialize_entry("type","timestamp_tz")?; }
            Value::Uuid(v)=>{
                let hex=v.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
                let uuid=format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..]);
                map.serialize_entry("type","uuid")?; map.serialize_entry("value",&uuid)?;
            }
        }
        map.end()
    }
}
struct Row<'a>(&'a [Value]);
impl Serialize for Row<'_> {
    fn serialize<S:Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        let mut seq=serializer.serialize_seq(Some(self.0.len()))?;
        for value in self.0 { seq.serialize_element(&Cell(value))?; }
        seq.end()
    }
}
struct Rows<'a>(&'a [Vec<Value>]);
impl Serialize for Rows<'_> {
    fn serialize<S:Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        let mut seq=serializer.serialize_seq(Some(self.0.len()))?;
        for row in self.0 { seq.serialize_element(&Row(row))?; }
        seq.end()
    }
}
struct ResultLine<'a> { statement:usize,result:&'a SqlResultSet }
impl Serialize for ResultLine<'_> {
    fn serialize<S:Serializer>(&self,serializer:S)->Result<S::Ok,S::Error> {
        match self.result {
            SqlResultSet::Command { affected }=>{
                let mut map=serializer.serialize_map(Some(3))?; map.serialize_entry("affected",affected)?;
                map.serialize_entry("kind","command")?; map.serialize_entry("statement",&self.statement)?; map.end()
            }
            SqlResultSet::Rows { columns,rows }=>{
                let mut map=serializer.serialize_map(Some(4))?; map.serialize_entry("columns",columns)?;
                map.serialize_entry("kind","rows")?; map.serialize_entry("rows",&Rows(rows))?;
                map.serialize_entry("statement",&self.statement)?; map.end()
            }
        }
    }
}
struct LineBuffer { bytes:Vec<u8>,limit:usize,exceeded:bool }
impl Write for LineBuffer {
    fn write(&mut self,bytes:&[u8])->io::Result<usize> {
        if self.bytes.len().checked_add(bytes.len()).is_none_or(|n|n>self.limit) {
            self.exceeded=true; return Err(io::Error::other("SQL output byte limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self)->io::Result<()> { Ok(()) }
}
pub(crate) fn output(writer:&mut impl Write,number:usize,result:SqlResultSet,limit:usize)->Result<()> {
    let mut line=LineBuffer { bytes:Vec::new(),limit,exceeded:false };
    let encoded=(||->Result<()> {
        serde_json::to_writer(&mut line,&ResultLine { statement:number,result:&result })?;
        line.write_all(b"\n")?; Ok(())
    })();
    anyhow::ensure!(!line.exceeded,"statement {number} output byte limit exceeded (limit {limit})");
    encoded.with_context(||format!("statement {number} output encoding failed"))?;
    writer.write_all(&line.bytes).with_context(||format!("statement {number} output write failed"))?;
    writer.flush().with_context(||format!("statement {number} output flush failed"))?; Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json,Value as Json};
    // Independent oracle: the previous public JSON profile used owned JSON maps.
    fn legacy(value:&Value)->Json {
        match value {
            Value::Null=>json!({"type":"null","value":null}),
            Value::Boolean(v)=>json!({"type":"boolean","value":v}),
            Value::Int32(v)=>json!({"type":"int32","value":v}),
            Value::Int64(v)=>json!({"type":"int64","value":v.to_string()}),
            Value::Decimal(v)=>json!({"type":"decimal","unscaled":v.to_string()}),
            Value::String(v)=>json!({"type":"text","value":v}),
            Value::TimestampTz(v)=>json!({"type":"timestamp_tz","microseconds":v.to_string()}),
            Value::Uuid(v)=>{
                let hex=v.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
                json!({"type":"uuid","value":format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..])})
            }
        }
    }
    #[test]
    fn borrowed_encoding_matches_original_profile_bytes_for_all_types_and_commands() {
        let values=vec![Value::Null,Value::Boolean(true),Value::Boolean(false),Value::Int32(i32::MIN),Value::Int64(i64::MAX),Value::Decimal(i128::MIN),Value::TimestampTz(i64::MIN),Value::Uuid([0xab;16]),Value::String("é\n\0\"\\".into())];
        let columns=(0..values.len()).map(|n|format!("col{n}")).collect::<Vec<_>>();
        let expected=json!({"statement":7,"kind":"rows","columns":columns,"rows":[values.iter().map(legacy).collect::<Vec<_>>()]});
        let mut old=serde_json::to_vec(&expected).unwrap(); old.push(b'\n'); let mut actual=Vec::new();
        output(&mut actual,7,SqlResultSet::Rows { columns,rows:vec![values] },DEFAULT_OUTPUT_BYTES).unwrap(); assert_eq!(actual,old);
        let mut actual=Vec::new(); output(&mut actual,2,SqlResultSet::Command { affected:13 },DEFAULT_OUTPUT_BYTES).unwrap();
        let mut old=serde_json::to_vec(&json!({"statement":2,"kind":"command","affected":13})).unwrap(); old.push(b'\n'); assert_eq!(actual,old);
    }
    #[test]
    fn exact_limit_includes_newline_and_refusal_preserves_prior_output() {
        let command=||SqlResultSet::Command { affected:0 };
        let mut expected=Vec::new(); output(&mut expected,1,command(),1000).unwrap();
        let mut actual=Vec::new(); output(&mut actual,1,command(),expected.len()).unwrap(); assert_eq!(actual,expected);
        for limit in [0,expected.len()-1] {
            let mut prior=b"prior complete line\n".to_vec(); let before=prior.clone();
            let error=output(&mut prior,1,command(),limit).unwrap_err(); assert!(error.to_string().contains("output byte limit exceeded")); assert_eq!(prior,before);
        }
    }
    #[test]
    fn escaped_text_amplification_is_charged_before_any_json_line_is_emitted() {
        let rows=||SqlResultSet::Rows { columns:vec!["text".into()],rows:vec![vec![Value::String("\0".repeat(256))]] };
        let mut output_bytes=Vec::new(); assert!(output(&mut output_bytes,1,rows(),512).is_err()); assert!(output_bytes.is_empty());
        output(&mut output_bytes,1,rows(),10000).unwrap(); assert!(output_bytes.len()>256*6);
        let parsed:Json=serde_json::from_slice(&output_bytes).unwrap(); assert_eq!(parsed["rows"][0][0]["value"],"\0".repeat(256));
    }
    struct Broken { flush:bool,bytes:Vec<u8> }
    impl Write for Broken {
        fn write(&mut self,bytes:&[u8])->io::Result<usize> {
            if !self.flush { return Err(io::ErrorKind::BrokenPipe.into()); }
            self.bytes.extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self)->io::Result<()> { Err(io::ErrorKind::BrokenPipe.into()) }
    }
    #[test]
    fn sink_write_and_flush_errors_keep_statement_context() {
        for flush in [false,true] {
            let mut sink=Broken { flush,bytes:Vec::new() };
            let error=output(&mut sink,3,SqlResultSet::Command { affected:1 },1000).unwrap_err();
            assert!(error.to_string().contains(if flush { "statement 3 output flush failed" } else { "statement 3 output write failed" }));
            assert_eq!(!sink.bytes.is_empty(),flush);
        }
    }
}
