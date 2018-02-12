use std::io::{self, Write};

use stdlog::Record;
use env_logger::Formatter;
use serde::ser::{Serialize, SerializeMap, Serializer};

use current_logger;
use ctxt::Ctxt;

pub fn format() -> impl Fn(&mut Formatter, &Record) -> io::Result<()> {
    |buf, record| {
        current_logger().scope(|mut scope| do catch {
            write!(
                buf,
                "{}: {}: {}",
                record.level(),
                buf.timestamp(),
                record.args()
            )?;

            if let Some(ctxt) = scope.current() {
                let mut value_style = buf.style();

                value_style.set_bold(true);

                write!(buf, ": (")?;

                let mut first = true;
                for (k, v) in ctxt.properties() {
                    if first {
                        first = false;
                        write!(buf, "{}: {}", k, value_style.value(v))?;
                    } else {
                        write!(buf, ", {}: {}", k, value_style.value(v))?;
                    }
                }

                write!(buf, ")")?;
            }

            writeln!(buf)?;

            Ok(())
        })
    }
}

#[allow(unused)]
pub(crate) struct Log<'a, 'b> {
    ctxt: Option<&'a Ctxt>,
    record: Record<'b>,
}

impl<'a, 'b> Log<'a, 'b> {
    #[cfg(test)]
    pub(crate) fn new(ctxt: Option<&'a Ctxt>, record: Record<'b>) -> Self {
        Log { ctxt, record }
    }
}

impl<'a, 'b> Serialize for Log<'a, 'b> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct SerializeLog<'a, 'b> {
            #[serde(serialize_with = "serialize_msg")]
            msg: &'a Record<'a>,
            #[serde(serialize_with = "serialize_ctxt")]
            ctxt: Option<&'b Ctxt>,
        }

        fn serialize_ctxt<S>(ctxt: &Option<&Ctxt>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if let Some(ref ctxt) = *ctxt {
                let properties = ctxt.properties();

                let mut map = serializer.serialize_map(Some(properties.len()))?;

                for (k, v) in properties.iter() {
                    map.serialize_entry(k, v)?;
                }

                map.end()
            } else {
                serializer.serialize_none()
            }
        }

        fn serialize_msg<'a, S>(record: &Record<'a>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.collect_str(record.args())
        }

        let log = SerializeLog {
            msg: &self.record,
            ctxt: self.ctxt.clone(),
        };

        log.serialize(serializer)
    }
}
