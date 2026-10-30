//! The combination matrix's generator: every pair of values that can meet, the whole product of selected interacting
//! dimensions, and seeded random combinations beyond them; each case named so that it can
//! be run again alone.
//!
//! A matrix is a list of dimensions, each a name and its values, and a rule that says which combinations can meet at
//! all (a value of one dimension that makes another meaningless, a pair the product cannot hold). Three kinds of cases
//! come from it:
//!
//! - `pairs`: every pair of values of two dimensions that some valid combination holds is in at least one case. The
//!   cover is greedy: each new case is the best, by the number of pairs it adds, of seeded candidates that include one
//!   candidate built around the first pair still missing, so that every step adds at least one pair.
//! - `full`: the whole product of a few named dimensions (a triple the decisions say interacts), each combination
//!   completed with a valid choice of the other dimensions.
//! - `random`: further valid combinations drawn uniformly with a seed.
//!
//! Every case is written `<matrix>/<kind>/<seed>/<index>` followed by its values, and is replayed with
//! `ORCH_MATRIX_CASE=<matrix>/<kind>/<seed>/<index>` (the other cases are skipped). `ORCH_MATRIX_SEED=<number>` (or
//! `time`, which takes the clock and prints it) replaces the seed of the random cases, and `ORCH_MATRIX_RANDOM=<n>`
//! how many of them are drawn. The generator uses only the standard library, so that every crate's tests can include
//! this file by its path.
#![allow(dead_code, reason = "each matrix uses some of what is here")]

use std::collections::HashSet;
use std::fmt::Write as _;

/// A deterministic generator of numbers (SplitMix64): the same seed gives the same cases on every machine.
#[derive(Debug, Clone)]
pub struct Seeded(u64);

impl Seeded {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `bound` (which must be above zero).
    pub fn below(&mut self, bound: usize) -> usize {
        // The modulo's bias is below 2^-40 for the bounds used here; nothing depends on it.
        usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// One dimension: its name and its values.
#[derive(Debug, Clone, Copy)]
pub struct Dim {
    pub name: &'static str,
    pub values: &'static [&'static str],
}

#[must_use]
pub const fn dim(name: &'static str, values: &'static [&'static str]) -> Dim {
    Dim { name, values }
}

/// One combination, by the index of each dimension's value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Row(pub Vec<u8>);

/// What the rule of a matrix is asked about: a whole combination.
pub struct Pick<'a> {
    dims: &'a [Dim],
    row: &'a [u8],
}

impl Pick<'_> {
    /// The value of a dimension.
    #[must_use]
    pub fn get(&self, name: &str) -> &'static str {
        let at = self
            .dims
            .iter()
            .position(|dim| dim.name == name)
            .unwrap_or_else(|| panic!("the matrix has no dimension {name}"));
        self.dims[at].values[usize::from(self.row[at])]
    }

    #[must_use]
    pub fn is(&self, name: &str, value: &str) -> bool {
        self.get(name) == value
    }
}

/// A case to run: its name and its values.
#[derive(Debug, Clone)]
pub struct Case {
    pub matrix: &'static str,
    pub kind: &'static str,
    pub seed: u64,
    pub index: usize,
    dims: Vec<Dim>,
    row: Vec<u8>,
}

impl Case {
    /// The value of a dimension.
    #[must_use]
    pub fn get(&self, name: &str) -> &'static str {
        Pick {
            dims: &self.dims,
            row: &self.row,
        }
        .get(name)
    }

    #[must_use]
    pub fn is(&self, name: &str, value: &str) -> bool {
        self.get(name) == value
    }

    /// `<matrix>/<kind>/<seed>/<index>`, which `ORCH_MATRIX_CASE` takes.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}/{}/{}/{}", self.matrix, self.kind, self.seed, self.index)
    }

    /// The case's name and values, for a report.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut text = self.id();
        for (at, dim) in self.dims.iter().enumerate() {
            let _ = write!(
                text,
                " {}={}",
                dim.name,
                dim.values[usize::from(self.row[at])]
            );
        }
        text
    }

    /// A number of this case's own, for names that have to differ from case to case.
    #[must_use]
    pub fn tag(&self) -> String {
        format!("{}{}", &self.kind[..1], self.index)
    }
}

/// The largest product that is enumerated whole; a larger one is sampled.
const ENUMERATE_UP_TO: u64 = 2_000_000;
/// How many combinations are sampled from a larger product.
const SAMPLED: usize = 200_000;
/// How many seeded candidates each step of the pair cover compares.
const CANDIDATES: usize = 48;

/// A matrix: its dimensions and the rule of which combinations can meet.
pub struct Matrix {
    pub name: &'static str,
    pub dims: Vec<Dim>,
    valid: Vec<Row>,
}

impl Matrix {
    /// The matrix and every valid combination (or a large seeded sample of them).
    pub fn new(name: &'static str, dims: &[Dim], rule: impl Fn(&Pick<'_>) -> bool) -> Self {
        assert!(
            dims.iter()
                .all(|dim| !dim.values.is_empty() && dim.values.len() < 256)
        );
        let product: u64 = dims
            .iter()
            .map(|dim| u64::try_from(dim.values.len()).unwrap_or(u64::MAX))
            .fold(1, u64::saturating_mul);
        let mut valid = Vec::new();
        let keep = |row: &[u8], valid: &mut Vec<Row>| {
            if rule(&Pick { dims, row }) {
                valid.push(Row(row.to_vec()));
            }
        };
        if product <= ENUMERATE_UP_TO {
            let mut row = vec![0_u8; dims.len()];
            loop {
                keep(&row, &mut valid);
                // The next combination, the last dimension fastest.
                let mut at = dims.len();
                loop {
                    if at == 0 {
                        return Self::made(name, dims, valid);
                    }
                    at -= 1;
                    row[at] += 1;
                    if usize::from(row[at]) < dims[at].values.len() {
                        break;
                    }
                    row[at] = 0;
                }
            }
        }
        let mut seeded = Seeded::new(0x51_6d61_7472_6978);
        let mut seen = HashSet::new();
        for _ in 0..SAMPLED {
            let row: Vec<u8> = dims
                .iter()
                .map(|dim| u8::try_from(seeded.below(dim.values.len())).unwrap_or(0))
                .collect();
            if seen.insert(row.clone()) {
                keep(&row, &mut valid);
            }
        }
        Self::made(name, dims, valid)
    }

    fn made(name: &'static str, dims: &[Dim], valid: Vec<Row>) -> Self {
        assert!(
            !valid.is_empty(),
            "the matrix {name} has no valid combination"
        );
        Self {
            name,
            dims: dims.to_vec(),
            valid,
        }
    }

    /// How many valid combinations were found.
    #[must_use]
    pub fn valid_count(&self) -> usize {
        self.valid.len()
    }

    fn case(&self, kind: &'static str, seed: u64, index: usize, row: &Row) -> Case {
        Case {
            matrix: self.name,
            kind,
            seed,
            index,
            dims: self.dims.clone(),
            row: row.0.clone(),
        }
    }

    fn pairs_of(row: &Row) -> impl Iterator<Item = (usize, u8, usize, u8)> + '_ {
        (0..row.0.len())
            .flat_map(move |i| (i + 1..row.0.len()).map(move |j| (i, row.0[i], j, row.0[j])))
    }

    /// Every pair of values that can meet, each in at least one case.
    #[must_use]
    pub fn pairs(&self, seed: u64) -> Vec<Case> {
        let mut missing: HashSet<(usize, u8, usize, u8)> = HashSet::new();
        for row in &self.valid {
            missing.extend(Self::pairs_of(row));
        }
        let mut seeded = Seeded::new(seed);
        let mut chosen: Vec<Row> = Vec::new();
        while !missing.is_empty() {
            // The first missing pair, in a fixed order, so that the cover is the same on every run.
            let target = *missing.iter().min().expect("a pair is missing");
            let holding: Vec<&Row> = self
                .valid
                .iter()
                .filter(|row| row.0[target.0] == target.1 && row.0[target.2] == target.3)
                .collect();
            let mut candidates: Vec<&Row> = (0..CANDIDATES / 2)
                .map(|_| *seeded.pick(&holding))
                .collect();
            candidates.extend((0..CANDIDATES / 2).map(|_| seeded.pick(&self.valid)));
            let best = candidates
                .into_iter()
                .max_by_key(|row| {
                    Self::pairs_of(row)
                        .filter(|pair| missing.contains(pair))
                        .count()
                })
                .expect("a candidate")
                .clone();
            for pair in Self::pairs_of(&best) {
                missing.remove(&pair);
            }
            chosen.push(best);
        }
        chosen
            .iter()
            .enumerate()
            .map(|(index, row)| self.case("pairs", seed, index, row))
            .collect()
    }

    /// The whole product of the named dimensions, each combination that can meet completed with a valid choice of the
    /// others.
    #[must_use]
    pub fn full(&self, names: &[&str], seed: u64) -> Vec<Case> {
        let at: Vec<usize> = names
            .iter()
            .map(|name| {
                self.dims
                    .iter()
                    .position(|dim| dim.name == *name)
                    .unwrap_or_else(|| panic!("the matrix {} has no dimension {name}", self.name))
            })
            .collect();
        let mut seeded = Seeded::new(seed);
        let mut groups: Vec<(Vec<u8>, Vec<&Row>)> = Vec::new();
        for row in &self.valid {
            let key: Vec<u8> = at.iter().map(|&i| row.0[i]).collect();
            match groups.iter_mut().find(|(seen, _)| *seen == key) {
                Some((_, rows)) => rows.push(row),
                None => groups.push((key, vec![row])),
            }
        }
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        groups
            .iter()
            .enumerate()
            .map(|(index, (_, rows))| {
                // The group holds references: name the element type, and the `&&Row` coerces to `&Row`.
                let row: &Row = seeded.pick::<&Row>(rows.as_slice());
                self.case("full", seed, index, row)
            })
            .collect()
    }

    /// `count` further valid combinations drawn with `seed`.
    #[must_use]
    pub fn random(&self, count: usize, seed: u64) -> Vec<Case> {
        let mut seeded = Seeded::new(seed);
        (0..count)
            .map(|index| self.case("random", seed, index, seeded.pick(&self.valid)))
            .collect()
    }

    /// The pair cover, the full products of each set of interacting dimensions, and the random cases, with the seeds
    /// the environment may replace; then only the one case `ORCH_MATRIX_CASE` names, if it names one of these.
    #[must_use]
    pub fn cases(
        &self,
        pair_seed: u64,
        interacting: &[&[&str]],
        random_seed: u64,
        random: usize,
    ) -> Vec<Case> {
        let random_seed = random_seed_from_environment(random_seed);
        let random = std::env::var("ORCH_MATRIX_RANDOM")
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(random);
        let mut cases = self.pairs(pair_seed);
        for (n, names) in interacting.iter().enumerate() {
            // Each product has a seed of its own, so that its cases have names of their own.
            cases.extend(self.full(names, pair_seed + 1 + u64::try_from(n).unwrap_or(0)));
        }
        cases.extend(self.random(random, random_seed));
        eprintln!(
            "matrix {}: {} dimensions, {} valid combinations, {} pair cases, {} full cases, {} random cases (seed {random_seed})",
            self.name,
            self.dims.len(),
            self.valid.len(),
            cases.iter().filter(|case| case.kind == "pairs").count(),
            cases.iter().filter(|case| case.kind == "full").count(),
            cases.iter().filter(|case| case.kind == "random").count(),
        );
        only_the_named_case(cases)
    }
}

/// The random seed: `ORCH_MATRIX_SEED` when it is set (a number, or `time` for the clock's), else `default`.
#[must_use]
pub fn random_seed_from_environment(default: u64) -> u64 {
    match std::env::var("ORCH_MATRIX_SEED").ok().as_deref() {
        Some("time") => {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(default, |since| since.as_secs());
            eprintln!("ORCH_MATRIX_SEED=time chose the seed {seed}");
            seed
        }
        Some(number) => number.parse().unwrap_or(default),
        None => default,
    }
}

/// The cases left when `ORCH_MATRIX_CASE` names one: that one alone (or none of these, when it names another
/// matrix's).
#[must_use]
pub fn only_the_named_case(cases: Vec<Case>) -> Vec<Case> {
    match std::env::var("ORCH_MATRIX_CASE") {
        Ok(wanted) if !wanted.is_empty() => cases
            .into_iter()
            .filter(|case| case.id() == wanted)
            .collect(),
        _ => cases,
    }
}

/// What the cases of a matrix came to: each failure with the case, what was expected (and where that is written) and
/// what happened.
#[derive(Debug, Default)]
pub struct Report {
    ran: usize,
    failures: Vec<String>,
}

impl Report {
    pub fn passed(&mut self) {
        self.ran += 1;
    }

    pub fn failed(&mut self, case: &Case, expected: &str, source: &str, got: &str) {
        self.ran += 1;
        eprintln!("FAILED {}: expected {expected}; got {got}", case.id());
        self.failures.push(format!(
            "{}\n    expected: {expected}\n    source:   {source}\n    got:      {got}",
            case.describe()
        ));
    }

    /// One case's outcome: a pass, or a failure with its words.
    pub fn record(&mut self, case: &Case, outcome: Result<(), Failure>) {
        match outcome {
            Ok(()) => self.passed(),
            Err(failure) => self.failed(case, &failure.expected, failure.source, &failure.got),
        }
    }

    /// Fails the test with every failure, after all cases ran.
    pub fn finish(self, matrix: &str) {
        eprintln!(
            "matrix {matrix}: {} cases ran, {} failed",
            self.ran,
            self.failures.len()
        );
        assert!(
            self.failures.is_empty(),
            "matrix {matrix}: {} of {} cases failed (replay one with ORCH_MATRIX_CASE=<id>):\n{}",
            self.failures.len(),
            self.ran,
            self.failures.join("\n")
        );
    }
}

/// Why one case failed.
#[derive(Debug)]
pub struct Failure {
    pub expected: String,
    pub source: &'static str,
    pub got: String,
}

/// The stack of a thread that runs a case: the debug builds of the engine and the API need more than the 2 MiB a
/// thread is given by default (their deepest futures overflow it).
pub const CASE_STACK: usize = 32 << 20;

/// Runs the cases, `at_once` side by side, each on a thread of its own (with [`CASE_STACK`]): `run` builds what the
/// case needs (a runtime, a world) and says whether it held. A case that panics fails with the panic's words.
/// Threads, and not tasks of one runtime, so that a case that blocks its thread (making keys, say) holds up no other.
pub fn run_cases<F>(cases: &[Case], at_once: usize, source: &'static str, run: F) -> Report
where
    F: Fn(&Case) -> Result<(), Failure> + Send + Sync + 'static,
{
    let run = std::sync::Arc::new(run);
    let mut report = Report::default();
    for chunk in cases.chunks(at_once.max(1)) {
        let running: Vec<_> = chunk
            .iter()
            .cloned()
            .map(|case| {
                let run = std::sync::Arc::clone(&run);
                let own = case.clone();
                let handle = std::thread::Builder::new()
                    .name(case.id())
                    .stack_size(CASE_STACK)
                    .spawn(move || run(&own))
                    .expect("a thread for a case");
                (case, handle)
            })
            .collect();
        for (case, handle) in running {
            match handle.join() {
                Ok(outcome) => report.record(&case, outcome),
                Err(payload) => report.failed(
                    &case,
                    "the case runs to its end",
                    source,
                    &panic_words(payload),
                ),
            }
        }
    }
    report
}

/// The words of a case that panicked (a helper of the rig gave up waiting, say), for its failure.
#[must_use]
pub fn panic_words(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|words| (*words).to_owned())
        })
        .unwrap_or_else(|| "a panic without words".to_owned())
}

/// A failure of a case.
#[must_use]
pub fn failure(
    expected: impl Into<String>,
    source: &'static str,
    got: impl Into<String>,
) -> Failure {
    Failure {
        expected: expected.into(),
        source,
        got: got.into(),
    }
}

/// Checks that `got` is `expected`, or says what the case expected and where that is written.
pub fn same<T: PartialEq + std::fmt::Debug>(
    expected: &T,
    got: &T,
    source: &'static str,
) -> Result<(), Failure> {
    if expected == got {
        Ok(())
    } else {
        Err(failure(format!("{expected:?}"), source, format!("{got:?}")))
    }
}

#[cfg(test)]
mod kit_tests {
    use super::*;

    const A: Dim = dim("a", &["0", "1", "2"]);
    const B: Dim = dim("b", &["x", "y"]);
    const C: Dim = dim("c", &["p", "q", "r"]);

    #[test]
    fn the_pair_cover_holds_every_pair_that_can_meet_and_no_pair_the_rule_forbids() {
        let matrix = Matrix::new("kit", &[A, B, C], |pick| {
            !(pick.is("a", "2") && pick.is("c", "r"))
        });
        let cases = matrix.pairs(1);
        let mut covered = HashSet::new();
        for case in &cases {
            assert!(!(case.is("a", "2") && case.is("c", "r")));
            for (x, y) in [("a", "b"), ("a", "c"), ("b", "c")] {
                covered.insert((x, case.get(x), y, case.get(y)));
            }
        }
        // 3*2 + (3*3 - 1) + 2*3 pairs can meet.
        assert_eq!(covered.len(), 6 + 8 + 6);
        assert_eq!(
            matrix
                .pairs(1)
                .iter()
                .map(Case::describe)
                .collect::<Vec<_>>(),
            cases.iter().map(Case::describe).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_full_product_has_each_combination_of_its_dimensions_once() {
        let matrix = Matrix::new("kit", &[A, B, C], |_| true);
        assert_eq!(matrix.full(&["a", "c"], 3).len(), 9);
        assert_eq!(matrix.random(5, 9).len(), 5);
    }
}
