use crate::error::Error;
use auxon_sdk::{
    api::{AttrKey, AttrVal, Nanoseconds, TimelineId},
    ingest_client::{dynamic::DynamicIngestClient, IngestClient},
    ingest_protocol::InternedAttrKey,
    reflector_config::AttrKeyEqValuePair,
};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tokio::runtime::{self, Runtime};
use tracing::debug;
use uuid::Uuid;

mod error;

#[pymodule]
fn modality_client(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ModalityClient>()?;

    Ok(())
}

type SuiteName = String;
type TestName = String;

#[pyclass]
pub struct ModalityClient {
    rt: Runtime,
    active_suite: Option<SuiteName>,
    tests_to_timelines: HashMap<TestName, TimelineId>,
    extra_timeline_attrs: HashMap<AttrKey, AttrVal>,
    global_nonce: u32,
    ordering: u128,
    client: DynamicIngestClient,
    attrs: HashMap<String, InternedAttrKey>,
}

const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const RUN_ID_ENV_VAR: &str = "MODALITY_RUN_ID";

#[pymethods]
impl ModalityClient {
    #[new]
    pub fn new(additional_timeline_attrs: Option<Vec<String>>) -> Result<ModalityClient, Error> {
        tracing_subscriber::fmt::init();
        let rt = runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = rt
            .block_on(IngestClient::connect_with_standard_config(
                CLIENT_TIMEOUT,
                None,
                None,
            ))?
            .into();
        let mut extra_timeline_attrs = HashMap::new();
        for attr in additional_timeline_attrs.unwrap_or_default() {
            let kv = AttrKeyEqValuePair::from_str(&attr)?;
            extra_timeline_attrs.insert(kv.0, kv.1);
        }

        Ok(Self {
            rt,
            active_suite: None,
            tests_to_timelines: Default::default(),
            extra_timeline_attrs,
            global_nonce: 1,
            ordering: 0,
            client,
            attrs: Default::default(),
        })
    }

    pub fn on_suite_setup(&mut self, suite_name: &str) -> Result<(), Error> {
        if self.active_suite.is_some() {
            self.on_suite_teardown()?;
        }
        debug!(suite_name, "on_suite_setup");
        self.active_suite = Some(suite_name.into());
        Ok(())
    }

    pub fn on_suite_teardown(&mut self) -> Result<(), Error> {
        if let Some(suite_name) = self.active_suite.take() {
            debug!(suite_name, "on_suite_teardown");
            self.client.close_timeline();
            self.rt.block_on(self.client.flush())?;
        }
        Ok(())
    }

    pub fn on_test_setup(&mut self, test_name: &str) -> Result<(), Error> {
        let suite_name = self.active_suite.as_ref().ok_or(Error::NoSuiteActive)?;

        let mut timeline_is_new = false;
        let timeline_id = *self
            .tests_to_timelines
            .entry(test_name.to_owned())
            .or_insert_with(|| {
                timeline_is_new = true;
                TimelineId::allocate()
            });
        self.rt.block_on(self.client.open_timeline(timeline_id))?;

        if timeline_is_new {
            let mut attrs = HashMap::new();
            let run_id = if let Ok(env_val) = std::env::var(RUN_ID_ENV_VAR) {
                env_val.into()
            } else {
                Uuid::new_v4().to_string().into()
            };
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.name",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                "robot_framework".into(),
            );
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.robot_framework.suite.name",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                suite_name.into(),
            );
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.robot_framework.test.name",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                test_name.into(),
            );
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.id",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                timeline_id.into(),
            );
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.clock_style",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                "utc".into(),
            );
            attrs.insert(
                self.rt.block_on(declare_attr_key(
                    "timeline.run_id",
                    &mut self.client,
                    &mut self.attrs,
                ))?,
                run_id,
            );

            for (k, v) in self.extra_timeline_attrs.iter() {
                attrs.insert(
                    self.rt.block_on(declare_attr_key(
                        &format!("timeline.{}", k),
                        &mut self.client,
                        &mut self.attrs,
                    ))?,
                    v.clone(),
                );
            }

            self.rt.block_on(self.client.timeline_metadata(attrs))?;
        }

        event(
            self,
            [
                ("event.name", "test_setup".into()),
                ("event.suite.name", suite_name.into()),
                ("event.test.name", test_name.into()),
            ],
        )?;

        Ok(())
    }

    pub fn on_test_teardown(&mut self, test_name: &str) -> Result<(), Error> {
        let suite_name = self.active_suite.as_ref().ok_or(Error::NoSuiteActive)?;

        if let Some(timeline_id) = self.tests_to_timelines.remove(test_name) {
            self.rt.block_on(self.client.open_timeline(timeline_id))?;
            event(
                self,
                [
                    ("event.name", "test_teardown".into()),
                    ("event.suite.name", suite_name.into()),
                    ("event.test.name", test_name.into()),
                ],
            )?;
        }
        Ok(())
    }

    pub fn on_test_passed(&mut self, test_name: &str) -> Result<(), Error> {
        let suite_name = self.active_suite.as_ref().ok_or(Error::NoSuiteActive)?;

        if let Some(timeline_id) = self.tests_to_timelines.get(test_name) {
            self.rt.block_on(self.client.open_timeline(*timeline_id))?;
            event(
                self,
                [
                    ("event.name", "test_result".into()),
                    ("event.suite.name", suite_name.into()),
                    ("event.test.name", test_name.into()),
                    ("event.test.result", "passed".into()),
                    ("event.test.result.code", 0_i64.into()),
                ],
            )?;
        }
        Ok(())
    }

    pub fn on_test_failed(&mut self, test_name: &str) -> Result<(), Error> {
        let suite_name = self.active_suite.as_ref().ok_or(Error::NoSuiteActive)?;

        if let Some(timeline_id) = self.tests_to_timelines.get(test_name) {
            self.rt.block_on(self.client.open_timeline(*timeline_id))?;
            event(
                self,
                [
                    ("event.name", "test_result".into()),
                    ("event.suite.name", suite_name.into()),
                    ("event.test.name", test_name.into()),
                    ("event.test.result", "failed".into()),
                    ("event.test.result.code", 1_i64.into()),
                ],
            )?;
        }
        Ok(())
    }

    pub fn start_component(&mut self, component_name: &str) -> Result<u32, Error> {
        let nonce = self.global_nonce;
        self.global_nonce += 1;
        event(
            self,
            [
                ("event.name", "start_component".into()),
                ("event.nonce", nonce.into()),
                ("event.component_name", component_name.into()),
            ],
        )?;
        Ok(nonce)
    }
}

fn event<'a>(
    c: &mut ModalityClient,
    attrs: impl IntoIterator<Item = (&'a str, AttrVal)>,
) -> Result<(), Error> {
    let mut iattrs = HashMap::new();
    for kv in attrs.into_iter() {
        iattrs.insert(
            c.rt.block_on(declare_attr_key(kv.0, &mut c.client, &mut c.attrs))?,
            kv.1,
        );
    }
    iattrs.insert(
        c.rt.block_on(declare_attr_key(
            "event.timestamp",
            &mut c.client,
            &mut c.attrs,
        ))?,
        Nanoseconds::from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
        )
        .into(),
    );

    c.rt.block_on(c.client.event(c.ordering, iattrs))?;
    c.ordering += 1;
    Ok(())
}

async fn declare_attr_key(
    k: &str,
    client: &mut DynamicIngestClient,
    attrs: &mut HashMap<String, InternedAttrKey>,
) -> Result<InternedAttrKey, Error> {
    if let Some(ikey) = attrs.get(k) {
        Ok(*ikey)
    } else {
        let ikey = client.declare_attr_key(k.to_owned()).await?;
        attrs.insert(k.to_owned(), ikey);
        Ok(ikey)
    }
}
