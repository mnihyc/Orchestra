"""Meaningful allocation and actual-report reconciliation controls; no browser required."""
from copy import deepcopy
from collections import Counter
import unittest
import ci_web


def row(file, title, project="chromium", annotations=None, suites=None):
    return dict(id=file+title, file=file, project=project, titles=(suites or [])+[title],
                annotations=annotations or [], expected="passed", results=[])


class WebTests(unittest.TestCase):
    def inventory(self):
        rows = [row(ci_web.LAYOUT, "manifest")]
        for width in (1440, 400):
            for theme in ("light", "dark"):
                for locale in ("en", "zh-CN"):
                    rows += [row(ci_web.LAYOUT, "/"+str(i), suites=[f"layout rules {width}px {theme} {locale}"]) for i in range(40)]
        rows += [row("tests/e2e/history.spec.ts", str(i)) for i in range(4)]
        rows += [row("tests/e2e/native.spec.ts", str(i), annotations=[{"type":"serial","description":None}]) for i in range(6)]
        rows += [row("tests/performance/thread-scale.spec.ts", str(i)) for i in range(3)]
        rows += [row("tests/performance/request-budget.spec.ts", str(i)) for i in range(8)]
        rows += [row("tests/e2e/gallery.spec.ts", str(i), project="development") for i in range(3)]
        rows[0]["expected"] = "skipped"
        rows[0]["annotations"] = [{"type":"skip","description":"intentional"}]
        return rows

    def test_all_discovered_cases_once_with_serial_history_and_projects_preserved(self):
        rows = self.inventory()
        groups = ci_web.partition(rows)
        assigned = [r for g in groups for u in g["units"] for r in u["cases"]]
        self.assertEqual(Counter(map(ci_web.identity, rows)), Counter(map(ci_web.identity, assigned)))
        for file in ("tests/e2e/history.spec.ts", "tests/e2e/native.spec.ts"):
            self.assertEqual(sum(any(r["file"] == file for u in g["units"] for r in u["cases"]) for g in groups), 1)
        self.assertEqual(sum(g["kind"] == "performance" for g in groups), 4)
        self.assertTrue(all(g["seconds"] is None for g in groups if g["kind"] in {"performance","development"}))
        self.assertTrue(all(s.startswith("[") and " › " in s for g in groups for u in g["units"] for s in u["selectors"]))

    def receipts(self):
        rows = self.inventory(); groups = ci_web.partition(rows)
        data = dict(cases=[ci_web.declared(r) for r in rows], groups=groups, source={"head":"one"}, annotation_counts={"serial":6,"skip":1})
        receipts = []
        for group in groups:
            actual = [deepcopy(r) for u in group["units"] for r in u["cases"]]
            for r in actual:r["results"]=[dict(status=r["expected"], retry=0, duration=1)]
            receipts.append(dict(group=group["name"], attempt=1, cases=actual, source=data["source"], artifact="same", returncode=0, errors=[], elapsed_seconds=2))
        return data, receipts

    def test_actual_union_requires_every_named_case_and_preserves_intentional_skips(self):
        data, receipts = self.receipts()
        self.assertFalse(ci_web.reconcile(data, receipts)["errors"])
        for mutate in (lambda rs:rs.pop(), lambda rs:rs[0]["cases"].pop(), lambda rs:rs.append(deepcopy(rs[0]))):
            copy=deepcopy(receipts); mutate(copy)
            self.assertTrue(ci_web.reconcile(data,copy)["errors"])
        copy=deepcopy(receipts);copy[0]["source"]={"head":"wrong"}
        self.assertTrue(ci_web.reconcile(data,copy)["errors"])

    def test_extra_runtime_skip_or_mixed_artifact_cannot_manufacture_a_pass(self):
        data, receipts = self.receipts()
        copy=deepcopy(receipts);copy[0]["cases"][0]["results"]=[]
        self.assertTrue(ci_web.reconcile(data,copy)["errors"])
        copy=deepcopy(receipts);copy[0]["artifact"]="other"
        self.assertTrue(ci_web.reconcile(data,copy)["errors"])

    def test_latest_attempt_cannot_hide_a_new_failure_behind_an_old_pass(self):
        data, receipts = self.receipts()
        newest=deepcopy(receipts[0]);newest.update(attempt=2,returncode=1)
        self.assertTrue(ci_web.reconcile(data,receipts+[newest])["errors"])

    def test_unknown_project_and_duplicate_discovery_fail_closed(self):
        rows=self.inventory()
        with self.assertRaises(ValueError):ci_web.partition(rows+[rows[0]])
        rows[0]["project"]="new"
        with self.assertRaises(ValueError):ci_web.partition(rows)


if __name__ == "__main__":unittest.main()
