#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod fakes;

use std::thread;
use std::time::{Duration, Instant};

use crownos_config::system::ai::Device;

use super::{BenchmarkCache, BreakerSettings, Policy};
use crate::error::InferError;
use crate::registry::{Capability, Registry, Tier};
use crate::task::Task;
use fakes::{Behaviour, Harness, REGISTRY, SECOND, settings};

/// Generous, so that a loaded build machine does not fail a test.
const QUICK: Duration = Duration::from_secs(3);

#[test]
fn takes_the_best_candidate_that_the_probe_and_policy_allow() {
    let harness = Harness::new(&[Device::Vulkan, Device::Cpu], &[], settings());
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");

    let mut high_tier = settings();
    high_tier.policy.tier = Tier::High;
    let harness = Harness::new(&[Device::Cuda], &[], high_tier);
    assert_eq!(harness.describe(SECOND).unwrap(), "big@Cuda");
}

#[test]
fn on_battery_skips_candidates_that_need_mains_power() {
    let harness = Harness::new(&[Device::Cpu], &[], settings());
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Cpu");
    harness.router.set_policy(Policy {
        on_battery: true,
        ..Policy::default()
    });
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Unavailable(_))
    ));
}

#[test]
fn remote_needs_consent() {
    let harness = Harness::new(&[], &[], settings());
    assert!(harness.describe(SECOND).is_err());
    harness.router.set_policy(Policy {
        remote_consent: true,
        ..Policy::default()
    });
    assert_eq!(harness.describe(SECOND).unwrap(), "big@Remote");
}

#[test]
fn an_error_falls_through_and_opens_the_breaker() {
    let harness = Harness::all_devices();
    harness.llama.set("small", Device::Cuda, Behaviour::Fail);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(
        harness.calls(),
        ["small@Cuda", "small@Vulkan", "small@Vulkan"]
    );
}

#[test]
fn inconclusive_results_fall_through_without_opening_the_breaker() {
    for behaviour in [Behaviour::Busy, Behaviour::OwnTimeout] {
        let harness = Harness::all_devices();
        harness.llama.set("small", Device::Cuda, behaviour);
        assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
        assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
        assert_eq!(
            harness.calls(),
            ["small@Cuda", "small@Vulkan", "small@Cuda", "small@Vulkan"]
        );
    }
}

#[test]
fn an_inconclusive_half_open_trial_keeps_the_breaker_open() {
    let cooldown = Duration::from_millis(1500);
    let mut slow_cooldown = settings();
    slow_cooldown.breaker = BreakerSettings {
        cooldown,
        max_cooldown: cooldown * 4,
    };
    let harness = Harness::new(&[Device::Cuda, Device::Vulkan], &[], slow_cooldown);
    harness.llama.set("small", Device::Cuda, Behaviour::Fail);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");

    thread::sleep(cooldown + Duration::from_millis(200));
    harness.llama.set("small", Device::Cuda, Behaviour::Busy);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");

    thread::sleep(cooldown + Duration::from_millis(200));
    harness.llama.set("small", Device::Cuda, Behaviour::Answer);
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Cuda");
    assert_eq!(
        harness.calls(),
        [
            "small@Cuda",
            "small@Vulkan",
            "small@Cuda",
            "small@Vulkan",
            "small@Vulkan",
            "small@Cuda"
        ]
    );
}

#[test]
fn a_call_past_its_hang_limit_is_found_by_the_next_call() {
    let mut short_limit = settings();
    short_limit.unmeasured_call_limit = Duration::from_millis(300);
    let harness = Harness::new(&[Device::Cuda, Device::Vulkan], &[], short_limit);
    harness.llama.set("small", Device::Cuda, Behaviour::Hang);
    let started = Instant::now();
    assert!(matches!(
        harness.describe(Duration::from_millis(100)),
        Err(InferError::Timeout)
    ));
    assert!(started.elapsed() < QUICK);
    thread::sleep(Duration::from_millis(400));
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
    assert_eq!(harness.describe(SECOND).unwrap(), "small@Vulkan");
}

#[test]
fn a_call_slower_than_its_deadline_keeps_the_model_and_the_breaker_closed() {
    let harness = Harness::all_devices();
    harness.llama.set("small", Device::Cuda, Behaviour::Slow);
    assert!(matches!(
        harness.describe(Duration::from_millis(100)),
        Err(InferError::Timeout)
    ));
    assert_eq!(harness.describe(QUICK).unwrap(), "small@Cuda");
    assert_eq!(harness.llama.load_count(), 1);
}

#[test]
fn a_hung_call_with_a_known_latency_falls_through_before_the_deadline() {
    let harness = Harness::all_devices();
    harness.embed(SECOND).unwrap();
    harness.onnx.set("embed-a", Device::Cuda, Behaviour::Hang);
    let started = Instant::now();
    harness.embed(10 * SECOND).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        harness.calls(),
        ["embed-a@Cuda", "embed-a@Cuda", "embed-a@Cpu"]
    );
}

#[test]
fn a_large_batch_gets_a_hang_limit_in_proportion_to_its_size() {
    let harness = Harness::all_devices();
    harness
        .onnx
        .set("embed-a", Device::Cuda, Behaviour::SlowPerText);
    harness.embed(SECOND).unwrap();
    let large_batch: Vec<String> = (0..1500).map(|_| "a".to_owned()).collect();
    harness.embed_texts(large_batch, 20 * SECOND).unwrap();
    assert_eq!(harness.calls(), ["embed-a@Cuda", "embed-a@Cuda"]);
}

#[test]
fn concurrent_cold_calls_share_one_load() {
    let harness = Harness::all_devices();
    *harness.llama.load_time.lock().unwrap() = Duration::from_millis(300);
    thread::scope(|scope| {
        let calls: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| harness.describe(QUICK)))
            .collect();
        for call in calls {
            assert_eq!(call.join().unwrap().unwrap(), "small@Cuda");
        }
    });
    assert_eq!(harness.llama.load_count(), 1);
}

#[test]
fn a_load_slower_than_the_deadline_keeps_the_model_and_the_breaker_closed() {
    let harness = Harness::all_devices();
    *harness.llama.load_time.lock().unwrap() = Duration::from_millis(500);
    assert!(matches!(
        harness.describe(Duration::from_millis(50)),
        Err(InferError::Timeout)
    ));
    assert_eq!(harness.describe(QUICK).unwrap(), "small@Cuda");
    assert_eq!(harness.llama.load_count(), 1);
}

#[test]
fn open_breakers_answer_busy_with_a_retry_time() {
    let harness = Harness::new(&[Device::Cuda], &[], settings());
    harness.llama.set("small", Device::Cuda, Behaviour::Fail);
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Backend(_))
    ));
    assert!(matches!(
        harness.describe(SECOND),
        Err(InferError::Busy { retry_after }) if retry_after > Duration::from_secs(20)
    ));
}

#[test]
fn latency_orders_devices_only_inside_one_run_of_a_model() {
    let candidates = Registry::from_ron(REGISTRY).unwrap().tasks[&Task::SummarizeSegments].clone();
    let mut benchmarks = BenchmarkCache::default();
    for (candidate, millis) in candidates.iter().zip([50, 10, 5, 1, 1]) {
        benchmarks.record(
            Task::SummarizeSegments,
            candidate,
            Duration::from_millis(millis),
            1,
        );
    }
    let mut measured = settings();
    measured.benchmarks = benchmarks;
    measured.policy.remote_consent = true;
    let harness = Harness::new(&[Device::Cuda, Device::Vulkan, Device::Cpu], &[], measured);
    assert_eq!(
        harness.ranked(Task::SummarizeSegments),
        [
            "small@Vulkan",
            "small@Cuda",
            "big@Cuda",
            "small@Cpu",
            "big@Remote"
        ]
    );
}

#[test]
fn a_candidate_too_slow_for_the_deadline_is_skipped() {
    let candidates = Registry::from_ron(REGISTRY).unwrap().tasks[&Task::EmbedText].clone();
    let mut benchmarks = BenchmarkCache::default();
    benchmarks.record(Task::EmbedText, &candidates[0], Duration::from_secs(5), 2);
    let mut measured = settings();
    measured.benchmarks = benchmarks;
    let harness = Harness::new(&[], &[Device::Cuda, Device::Cpu], measured);
    harness.embed(SECOND).unwrap();
    assert_eq!(harness.calls(), ["embed-a@Cpu"]);
}

#[test]
fn embedding_never_falls_through_to_another_space() {
    let harness = Harness::all_devices();
    harness.onnx.set("embed-a", Device::Cuda, Behaviour::Fail);
    harness.onnx.set("embed-a", Device::Cpu, Behaviour::Fail);
    assert_eq!(
        harness.router.embed_space().unwrap().to_string(),
        "embed-a/OnnxRuntime/int8/4"
    );
    assert!(harness.embed(SECOND).is_err());
    assert_eq!(harness.calls(), ["embed-a@Cuda", "embed-a@Cpu"]);
}

#[test]
fn the_space_changes_only_when_its_build_cannot_run() {
    let harness = Harness::new(&[Device::Vulkan], &[], settings());
    assert_eq!(harness.embed(SECOND).unwrap(), "embed-a/LlamaCpp/Q8_0/4");
}

#[test]
fn idle_unload_keeps_the_resident_capabilities() {
    let harness = Harness::all_devices();
    harness.embed(SECOND).unwrap();
    harness.describe(SECOND).unwrap();
    assert_eq!(harness.onnx.load_count(), 1);
    assert_eq!(harness.llama.load_count(), 1);
    assert_eq!(
        harness
            .router
            .unload_idle(Duration::ZERO, &[Capability::Embed]),
        1
    );
    harness.embed(SECOND).unwrap();
    harness.describe(SECOND).unwrap();
    assert_eq!(harness.onnx.load_count(), 1);
    assert_eq!(harness.llama.load_count(), 2);
}
