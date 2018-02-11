use std::io::{self, Write};

use stdlog::Record;
use env_logger::Formatter;
use serde::ser::{Serialize, SerializeMap, Serializer};

use {logger, Scope};

pub fn format() -> impl Fn(&mut Formatter, &Record) -> io::Result<()> {
    |buf, record| {
        logger().get().scope(|scope| do catch {
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

pub struct Log<'a, 'b> {
    scope: Scope<'a>,
    record: Record<'b>,
}

impl<'a, 'b> Log<'a, 'b> {
    pub(crate) fn new(scope: Scope<'a>, record: Record<'b>) -> Self {
        Log { scope, record }
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
            #[serde(serialize_with = "serialize_scope")]
            scope: Scope<'b>,
        }

        fn serialize_scope<S>(scope: &Scope, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if let Some(ref ctxt) = scope.current() {
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
            scope: self.scope,
        };

        log.serialize(serializer)
    }
}
