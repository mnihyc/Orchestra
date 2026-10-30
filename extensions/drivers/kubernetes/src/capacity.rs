//! Aggregate capacity of compatible, ready and schedulable cluster workers.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Node;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::host::HostResources;

/// Kubernetes measures CPU in cores; Orch reports thousandths of a core.
const MILLIS_PER_CPU: u128 = 1_000;
/// Orch reports memory in mebibytes; Kubernetes memory quantities are bytes.
const BYTES_PER_MIB: u128 = 1_024 * 1_024;
/// Kubernetes quantities use decimal exponents, SI prefixes and binary prefixes with these bases.
const DECIMAL_BASE: u128 = 10;
const SI_PREFIX_BASE: u128 = 1_000;
const BINARY_PREFIX_BASE: u128 = 1_024;
/// Kubernetes caps quantity magnitude at the signed 64-bit maximum.
const MAX_QUANTITY: u128 = (1_u128 << 63) - 1;

fn unavailable() -> DriverError {
    DriverError::Unavailable("the cluster did not report usable worker capacity".into())
}

fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn scale(numerator: u128, denominator: u128, factor: u128, divisor: u128) -> Option<(u128, u128)> {
    if denominator == 0 || divisor == 0 {
        return None;
    }
    let cancel_left = gcd(numerator, divisor);
    let cancel_right = gcd(factor, denominator);
    Some((
        (numerator / cancel_left).checked_mul(factor / cancel_right)?,
        (denominator / cancel_right).checked_mul(divisor / cancel_left)?,
    ))
}

fn power(base: u128, exponent: u32) -> Result<u128, DriverError> {
    base.checked_pow(exponent).ok_or_else(unavailable)
}

/// Reads Kubernetes quantities as checked, nonnegative integer fractions.
fn fraction(quantity: &Quantity) -> Result<(u128, u128), DriverError> {
    let text = quantity.0.as_str();
    let text = text.strip_prefix('+').unwrap_or(text);
    let split = text
        .bytes()
        .position(|byte| !byte.is_ascii_digit() && byte != b'.')
        .unwrap_or(text.len());
    let (number, suffix) = text.split_at(split);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(unavailable());
    }
    let digits = format!("{whole}{fraction}");
    let mut numerator: u128 = digits.parse().map_err(|_| unavailable())?;
    let decimals = u32::try_from(fraction.len()).map_err(|_| unavailable())?;
    let mut denominator = power(DECIMAL_BASE, decimals)?;
    let (factor, divisor) = match suffix {
        "" => (1, 1),
        "n" => (1, power(SI_PREFIX_BASE, 3)?),
        "u" => (1, power(SI_PREFIX_BASE, 2)?),
        "m" => (1, SI_PREFIX_BASE),
        "k" => (SI_PREFIX_BASE, 1),
        "M" => (power(SI_PREFIX_BASE, 2)?, 1),
        "G" => (power(SI_PREFIX_BASE, 3)?, 1),
        "T" => (power(SI_PREFIX_BASE, 4)?, 1),
        "P" => (power(SI_PREFIX_BASE, 5)?, 1),
        "E" => (power(SI_PREFIX_BASE, 6)?, 1),
        "Ki" => (BINARY_PREFIX_BASE, 1),
        "Mi" => (power(BINARY_PREFIX_BASE, 2)?, 1),
        "Gi" => (power(BINARY_PREFIX_BASE, 3)?, 1),
        "Ti" => (power(BINARY_PREFIX_BASE, 4)?, 1),
        "Pi" => (power(BINARY_PREFIX_BASE, 5)?, 1),
        "Ei" => (power(BINARY_PREFIX_BASE, 6)?, 1),
        exponent if exponent.starts_with('e') || exponent.starts_with('E') => {
            let exponent: i32 = exponent[1..].parse().map_err(|_| unavailable())?;
            let magnitude = exponent.unsigned_abs();
            if exponent < 0 {
                (1, power(DECIMAL_BASE, magnitude)?)
            } else {
                (power(DECIMAL_BASE, magnitude)?, 1)
            }
        }
        _ => return Err(unavailable()),
    };
    (numerator, denominator) =
        scale(numerator, denominator, factor, divisor).ok_or_else(unavailable)?;
    Ok((numerator, denominator))
}

/// Reads Kubernetes quantities in host units; fractional capacity rounds down.
fn units(quantity: &Quantity, multiply: u128, divide: u128) -> Result<i64, DriverError> {
    let (mut numerator, mut denominator) = fraction(quantity)?;
    (numerator, denominator) =
        scale(numerator, denominator, multiply, divide).ok_or_else(unavailable)?;
    i64::try_from(numerator / denominator).map_err(|_| unavailable())
}

/// Refuse a claim size Kubernetes would cap or round, rather than silently changing the request.
pub(crate) fn positive(quantity: &Quantity) -> bool {
    let Ok((numerator, denominator)) = fraction(quantity) else {
        return false;
    };
    if numerator == 0 {
        return false;
    }
    let whole = numerator / denominator;
    if whole > MAX_QUANTITY || whole == MAX_QUANTITY && numerator % denominator != 0 {
        return false;
    }
    scale(numerator, denominator, 1_000, 1)
        .is_some_and(|(millis, denominator)| millis % denominator == 0)
}

/// Compares exact quantities without cross multiplication or rounding away a small difference.
pub(crate) fn at_least(actual: &Quantity, requested: &Quantity) -> Result<bool, DriverError> {
    use std::cmp::Ordering;
    let (mut left, mut left_denominator) = fraction(actual)?;
    let (mut right, mut right_denominator) = fraction(requested)?;
    let mut reversed = false;
    let order = loop {
        let comparison = (left / left_denominator).cmp(&(right / right_denominator));
        if comparison != Ordering::Equal {
            break if reversed {
                comparison.reverse()
            } else {
                comparison
            };
        }
        let left_remainder = left % left_denominator;
        let right_remainder = right % right_denominator;
        if left_remainder == 0 || right_remainder == 0 {
            let comparison = left_remainder.cmp(&right_remainder);
            break if reversed {
                comparison.reverse()
            } else {
                comparison
            };
        }
        (left, left_denominator) = (left_denominator, left_remainder);
        (right, right_denominator) = (right_denominator, right_remainder);
        reversed = !reversed;
    };
    Ok(order != Ordering::Less)
}

/// Counts only compatible workers the scheduler can use; unmeasured storage and load remain unknown.
pub(crate) fn aggregate(
    nodes: &[Node],
    selector: &BTreeMap<String, String>,
) -> Result<HostResources, DriverError> {
    if selector.get("kubernetes.io/os").map(String::as_str) != Some("linux")
        || !selector
            .get("kubernetes.io/arch")
            .is_some_and(|arch| ["amd64", "arm64"].contains(&arch.as_str()))
    {
        return Err(DriverError::InvalidSpec(
            "the worker pool must be limited to the node's Linux tool architecture".into(),
        ));
    }
    let mut cpus = 0_i64;
    let mut cpu_millis = 0_i64;
    let mut memory_mb = 0_i64;
    let mut eligible = false;
    for node in nodes {
        if crate::platform::scheduling_blocked(node)
            || !selector.iter().all(|(key, value)| {
                node.metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(key))
                    == Some(value)
            })
        {
            continue;
        }
        let Some(status) = &node.status else { continue };
        let ready = status.conditions.as_ref().is_some_and(|conditions| {
            let mut ready = conditions
                .iter()
                .filter(|condition| condition.type_ == "Ready");
            ready
                .next()
                .is_some_and(|condition| condition.status == "True")
                && ready.next().is_none()
        });
        if !ready {
            continue;
        }
        eligible = true;
        let capacity = status.capacity.as_ref().ok_or_else(unavailable)?;
        let allocatable = status.allocatable.as_ref().ok_or_else(unavailable)?;
        let total_cpu = units(capacity.get("cpu").ok_or_else(unavailable)?, 1, 1)?;
        let free_cpu = units(
            allocatable.get("cpu").ok_or_else(unavailable)?,
            MILLIS_PER_CPU,
            1,
        )?;
        let free_memory = units(
            allocatable.get("memory").ok_or_else(unavailable)?,
            1,
            BYTES_PER_MIB,
        )?;
        cpus = cpus.checked_add(total_cpu).ok_or_else(unavailable)?;
        cpu_millis = cpu_millis.checked_add(free_cpu).ok_or_else(unavailable)?;
        memory_mb = memory_mb.checked_add(free_memory).ok_or_else(unavailable)?;
    }
    if !eligible {
        return Err(DriverError::Unavailable(
            "no compatible schedulable worker is ready".into(),
        ));
    }
    Ok(HostResources {
        cpus,
        cpu_millis: Some(cpu_millis),
        memory_mb,
        // The shared model uses zero for capacity it has not measured; free/used remain absent.
        disk_gb: 0,
        cpu_percent: None,
        memory_used_mb: None,
        disk_used_gb: None,
        disk_free_gb: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(ready: bool, unschedulable: bool, arch: &str) -> Node {
        serde_json::from_value(json!({
            "metadata": {"labels": {"kubernetes.io/os":"linux", "kubernetes.io/arch":arch}},
            "spec": {"unschedulable":unschedulable},
            "status": {"conditions":[{"type":"Ready", "status":if ready {"True"} else {"False"}}],
                "capacity":{"cpu":"4"}, "allocatable":{"cpu":"3500m", "memory":"1536Mi"}}
        }))
        .unwrap()
    }

    #[test]
    fn capacity_excludes_unready_unschedulable_and_incompatible_workers() {
        let selector = crate::platform::worker_selector("linux", "x86_64").unwrap();
        let nodes = [
            node(true, false, "amd64"),
            node(false, false, "amd64"),
            node(true, true, "amd64"),
            node(true, false, "arm64"),
        ];
        let resources = aggregate(&nodes, &selector).unwrap();
        assert_eq!(
            (resources.cpus, resources.cpu_millis, resources.memory_mb),
            (4, Some(3500), 1536)
        );
        assert_eq!(resources.disk_gb, 0);
        assert_eq!(resources.disk_free_gb, None);
    }

    #[test]
    fn a_ready_but_tainted_or_pressured_worker_is_not_schedulable_capacity() {
        let selector = crate::platform::worker_selector("linux", "x86_64").unwrap();
        for effect in ["NoSchedule", "NoExecute"] {
            let mut worker = node(true, false, "amd64");
            worker.spec.as_mut().unwrap().taints = Some(vec![
                serde_json::from_value(json!({"key":"fixture.test/blocked","effect":effect}))
                    .unwrap(),
            ]);
            assert!(aggregate(&[worker], &selector).is_err());
        }
        for pressure in ["DiskPressure", "MemoryPressure", "PIDPressure"] {
            let mut worker = node(true, false, "amd64");
            worker
                .status
                .as_mut()
                .unwrap()
                .conditions
                .as_mut()
                .unwrap()
                .push(serde_json::from_value(json!({"type":pressure,"status":"True"})).unwrap());
            assert!(aggregate(&[worker], &selector).is_err());
        }
    }

    #[test]
    fn missing_capacity_is_unknown_instead_of_reported_as_zero() {
        let selector = crate::platform::worker_selector("linux", "x86_64").unwrap();
        let mut worker = node(true, false, "amd64");
        worker
            .status
            .as_mut()
            .unwrap()
            .allocatable
            .as_mut()
            .unwrap()
            .remove("memory");
        assert!(aggregate(&[worker], &selector).is_err());
        assert!(aggregate(&[node(false, false, "amd64")], &selector).is_err());
        let mut ambiguous = node(true, false, "amd64");
        let conditions = ambiguous
            .status
            .as_mut()
            .unwrap()
            .conditions
            .as_mut()
            .unwrap();
        conditions.push(conditions[0].clone());
        assert!(aggregate(&[ambiguous], &selector).is_err());
        assert!(aggregate(&[node(true, false, "amd64")], &BTreeMap::new()).is_err());
    }

    #[test]
    fn noncanonical_fractional_quantities_are_scaled_without_overstating_capacity() {
        assert_eq!(
            units(&Quantity("1.5".into()), MILLIS_PER_CPU, 1).unwrap(),
            1500
        );
        assert_eq!(
            units(&Quantity("3500m".into()), MILLIS_PER_CPU, 1).unwrap(),
            3500
        );
        assert_eq!(
            units(&Quantity("1536Ki".into()), 1, BYTES_PER_MIB).unwrap(),
            1
        );
        assert_eq!(
            units(&Quantity("1.5Gi".into()), 1, BYTES_PER_MIB).unwrap(),
            1536
        );
        assert_eq!(
            units(&Quantity("40805508Ki".into()), 1, BYTES_PER_MIB).unwrap(),
            39849
        );
        assert_eq!(
            units(&Quantity("129e6".into()), 1, BYTES_PER_MIB).unwrap(),
            123
        );
        assert_eq!(
            units(&Quantity("0.1m".into()), MILLIS_PER_CPU, 1).unwrap(),
            0
        );
        assert!(units(&Quantity("-1".into()), 1, 1).is_err());
        assert!(units(&Quantity("bad".into()), 1, 1).is_err());
        assert!(units(&Quantity("18446744073709551616".into()), 1, 1).is_err());
    }

    #[test]
    fn claim_quantities_compare_exactly_even_when_their_spelling_differs() {
        let q = |value: &str| Quantity(value.into());
        assert!(at_least(&q("1536Mi"), &q("1.5Gi")).unwrap());
        assert!(!at_least(&q("1535Mi"), &q("1.5Gi")).unwrap());
        assert!(!at_least(&q("0.1"), &q("0.2")).unwrap());
        assert!(at_least(&q("0.2"), &q("0.1")).unwrap());
        assert!(at_least(&q("8Ei"), &q("8000Pi")).unwrap());
        assert!(at_least(&q("bad"), &q("1Gi")).is_err());
        assert!(positive(&q("1Mi")));
        assert!(!positive(&q("0")));
        assert!(!positive(&q("not-a-quantity")));
        assert!(!positive(&q("0.1m")));
        assert!(!positive(&q("8Ei")));
        assert!(positive(&q("1m")));
        assert!(positive(&q("9223372036854775807")));
    }
}
