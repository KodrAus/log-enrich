use stdlog::Record;
use serde::ser::{Serialize, SerializeMap, Serializer};

use ctxt::Ctxt;

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
        struct SerializeLog<'a, 'b> where 'a: 'b {
            #[serde(serialize_with = "serialize_msg")]
            msg: &'b Record<'a>,
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