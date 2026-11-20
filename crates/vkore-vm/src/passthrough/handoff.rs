//! Transaction for driver handoff. A failed preparation is still a transaction
//! that must be rolled back; restoration never precedes closing guest handles.
pub(super) trait Driver {
    fn current(&self) -> Result<Option<String>, String>;
    fn set_override(&mut self, value: &str) -> Result<(), String>;
    fn set_reset(&mut self, value: &str) -> Result<(), String>;
    fn unbind(&mut self) -> Result<(), String>;
    fn probe(&mut self) -> Result<(), String>;
}
pub(super) struct Handoff<D: Driver> {
    pub driver: D,
    pub original_driver: Option<String>,
    pub original_override: String,
    pub original_reset: String,
    changed: bool,
}
impl<D: Driver> Handoff<D> {
    pub fn new(
        driver: D,
        original_driver: Option<String>,
        original_override: String,
        original_reset: String,
    ) -> Self {
        Self {
            driver,
            original_driver,
            original_override,
            original_reset,
            changed: false,
        }
    }
    pub fn bind(&mut self, reset: &str) -> Result<(), String> {
        if self.driver.current()? != self.original_driver {
            return Err("NPU host driver changed during preparation.".into());
        }
        self.changed = true;
        self.driver.set_reset(reset)?;
        self.driver.set_override("vfio-pci")?;
        if self.original_driver.as_deref() != Some("vfio-pci") {
            if self.original_driver.is_some() {
                self.driver.unbind()?;
            }
            self.driver.probe()?;
        }
        if self.driver.current()?.as_deref() != Some("vfio-pci") {
            return Err("NPU did not bind to VFIO.".into());
        }
        Ok(())
    }
    pub fn restore(&mut self) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        let current = self.driver.current()?;
        if current != self.original_driver
            && current.as_deref() != Some("vfio-pci")
            && current.is_some()
        {
            return Err(
                "NPU driver was changed externally; refusing to detach the new driver.".into(),
            );
        }
        let driver_result: Result<(), String> = (|| {
            if current != self.original_driver {
                if current.is_some() {
                    self.driver.unbind()?;
                }
                if let Some(driver) = &self.original_driver {
                    self.driver.set_override(driver)?;
                    self.driver.probe()?;
                }
            }
            if self.driver.current()? != self.original_driver {
                return Err("The original NPU host driver could not be restored.".into());
            }
            Ok(())
        })();
        // Restore the override even if reattachment fails, so a subsequent host
        // probe is not trapped on vfio-pci. Retain changed=true to permit retry.
        let reset_result = self.driver.set_reset(&self.original_reset);
        let override_result = self.driver.set_override(&self.original_override);
        driver_result?;
        reset_result?;
        override_result?;
        self.changed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Fake {
        current: Option<String>,
        over: String,
        reset: String,
        fail: Option<usize>,
        writes: usize,
        events: Vec<&'static str>,
    }
    impl Fake {
        fn write(&mut self, event: &'static str) -> Result<(), String> {
            self.events.push(event);
            self.writes += 1;
            if self.fail == Some(self.writes) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }
    }
    impl Driver for Fake {
        fn current(&self) -> Result<Option<String>, String> {
            Ok(self.current.clone())
        }
        fn set_override(&mut self, v: &str) -> Result<(), String> {
            self.write("override")?;
            self.over = v.into();
            Ok(())
        }
        fn set_reset(&mut self, v: &str) -> Result<(), String> {
            self.write("reset")?;
            self.reset = v.into();
            Ok(())
        }
        fn unbind(&mut self) -> Result<(), String> {
            self.write("unbind")?;
            self.current = None;
            Ok(())
        }
        fn probe(&mut self) -> Result<(), String> {
            self.write("probe")?;
            self.current = Some(self.over.clone());
            Ok(())
        }
    }
    fn transaction(driver: Option<&str>, fail: Option<usize>) -> Handoff<Fake> {
        Handoff::new(
            Fake {
                current: driver.map(str::to_owned),
                over: "".into(),
                reset: "flr pm bus".into(),
                fail,
                writes: 0,
                events: vec![],
            },
            driver.map(str::to_owned),
            "".into(),
            "flr pm bus".into(),
        )
    }
    #[test]
    fn restores_native_unbound_and_prebound_devices_and_is_idempotent() {
        for driver in [Some("intel_vpu"), Some("amdxdna"), Some("vfio-pci"), None] {
            let mut t = transaction(driver, None);
            t.bind("flr").unwrap();
            assert_eq!(t.driver.current.as_deref(), Some("vfio-pci"));
            t.restore().unwrap();
            assert_eq!(t.driver.current.as_deref(), driver);
            assert_eq!(t.driver.over, "");
            assert_eq!(t.driver.reset, "flr pm bus");
            let writes = t.driver.writes;
            t.restore().unwrap();
            assert_eq!(t.driver.writes, writes);
        }
    }
    #[test]
    fn every_preparation_write_failure_rolls_back_without_losing_original_state() {
        for fail in 1..=4 {
            let mut t = transaction(Some("intel_vpu"), Some(fail));
            assert!(t.bind("flr").is_err());
            t.driver.fail = None;
            t.restore().unwrap();
            assert_eq!(t.driver.current.as_deref(), Some("intel_vpu"));
            assert_eq!(t.driver.over, "");
            assert_eq!(t.driver.reset, "flr pm bus");
        }
    }
    #[test]
    fn failed_reattachment_restores_override_and_can_be_retried() {
        let mut t = transaction(Some("amdxdna"), None);
        t.bind("flr").unwrap();
        t.driver.fail = Some(t.driver.writes + 3); // native re-probe
        assert!(t.restore().is_err());
        assert_eq!(t.driver.over, "");
        t.driver.fail = None;
        t.restore().unwrap();
        assert_eq!(t.driver.current.as_deref(), Some("amdxdna"));
    }
    #[test]
    fn external_driver_changes_are_not_overwritten() {
        let mut t = transaction(Some("intel_vpu"), None);
        t.bind("flr").unwrap();
        t.driver.current = Some("another-driver".into());
        let writes = t.driver.writes;
        assert!(t.restore().is_err());
        assert_eq!(t.driver.writes, writes);
    }
}
