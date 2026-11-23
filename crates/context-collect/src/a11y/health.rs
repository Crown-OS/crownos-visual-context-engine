use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use atspi::ObjectRefOwned;

/// A hung object is left alone this long before it is asked again.
const HUNG_FOR: Duration = Duration::from_secs(30);
/// Past this many entries the oldest knowledge is dropped all at once.
const CAPACITY: usize = 4_096;

/// Objects that are gone or did not answer, so a read does not wait on them again.
#[derive(Debug, Default)]
pub(crate) struct ObjectHealth {
    dead: HashSet<ObjectRefOwned>,
    hung_until: HashMap<ObjectRefOwned, Instant>,
}

impl ObjectHealth {
    pub(crate) fn is_usable(&self, object: &ObjectRefOwned, now: Instant) -> bool {
        !self.dead.contains(object)
            && self
                .hung_until
                .get(object)
                .is_none_or(|until| *until <= now)
    }

    pub(crate) fn mark_dead(&mut self, object: &ObjectRefOwned) {
        if self.dead.len() >= CAPACITY {
            self.dead.clear();
        }
        self.dead.insert(object.clone());
    }

    pub(crate) fn mark_hung(&mut self, object: &ObjectRefOwned, now: Instant) {
        if self.hung_until.len() >= CAPACITY {
            self.hung_until.retain(|_, until| *until > now);
        }
        self.hung_until.insert(object.clone(), now + HUNG_FOR);
    }

    /// The application left the bus; its objects go with it.
    pub(crate) fn forget_application(&mut self, bus_name: &str) {
        self.dead
            .retain(|object| object.name_as_str() != Some(bus_name));
        self.hung_until
            .retain(|object, _| object.name_as_str() != Some(bus_name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &'static str, path: &'static str) -> ObjectRefOwned {
        ObjectRefOwned::from_static_str_unchecked(name, path)
    }

    #[test]
    fn a_dead_object_stays_dead_until_its_application_leaves() {
        let mut health = ObjectHealth::default();
        let now = Instant::now();
        let gone = object(":1.5", "/org/a11y/atspi/accessible/7");
        health.mark_dead(&gone);
        assert!(!health.is_usable(&gone, now + Duration::from_secs(3_600)));
        health.forget_application(":1.5");
        assert!(health.is_usable(&gone, now));
    }

    #[test]
    fn a_hung_object_is_asked_again_later() {
        let mut health = ObjectHealth::default();
        let now = Instant::now();
        let slow = object(":1.9", "/org/a11y/atspi/accessible/root");
        health.mark_hung(&slow, now);
        assert!(!health.is_usable(&slow, now + Duration::from_secs(1)));
        assert!(health.is_usable(&slow, now + HUNG_FOR));
        assert!(health.is_usable(&object(":1.9", "/other"), now));
    }
}
