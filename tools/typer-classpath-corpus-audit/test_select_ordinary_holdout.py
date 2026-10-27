import csv
import tempfile
import unittest
from pathlib import Path

import select_ordinary_holdout


class SelectOrdinaryHoldoutTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.inventory = self.root / "inventory.tsv"
        self.main = self.root / "main.tsv"
        self.output = self.root / "holdout.tsv"
        self.main.write_text("identity\nmain/A.scala#tree=1\n", encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def write_inventory(self, rows):
        header = "method\towner_path\tmethod_name\tline\trank_score\ttier\tparameter_kind\tfeatures\n"
        lines = [header.rstrip("\n")]
        lines.extend("\t".join(row) for row in rows)
        self.inventory.write_text("metadata\tignored\n" + "\n".join(lines) + "\n", encoding="utf-8")

    def test_selects_fixed_buckets_deterministically_and_excludes_main(self):
        rows = []
        for lower, upper, quota in select_ordinary_holdout.BUCKETS:
            score = lower
            for index in range(quota + 1):
                rows.append((f"scala/{lower}-{index}.scala#tree={index}", f"scala.Owner{lower}_{index}", "method", str(index), str(score), "A0", "ordinary", f"score-{lower}"))
        rows.extend([
            ("main/A.scala#tree=1", "scala.Main", "method", "1", "6", "A0", "ordinary", "main"),
            ("scala/contextual.scala#tree=1", "scala.Contextual", "method", "1", "6", "A0", "contextual", "contextual"),
            ("scala/a1.scala#tree=1", "scala.A1", "method", "1", "25", "A1", "ordinary", "A1"),
        ])
        self.write_inventory(rows)

        first = select_ordinary_holdout.select(self.inventory, self.main)
        second = select_ordinary_holdout.select(self.inventory, self.main)
        self.assertEqual([r["identity"] for r in first], [r["identity"] for r in second])
        self.assertEqual(len(first), 25)
        self.assertEqual(len({r["identity"].split("#tree=", 1)[0] for r in first}), 25)
        self.assertEqual(len({r["owner_path"] for r in first}), 25)
        self.assertNotIn("main/A.scala#tree=1", {r["identity"] for r in first})
        self.assertEqual({r["score_bucket"] for r in first}, {f"A0-{lo:02d}-{hi:02d}" for lo, hi, _ in select_ordinary_holdout.BUCKETS})

    def test_rejects_insufficient_bucket_supply(self):
        self.write_inventory([
            (f"scala/{index}.scala#tree={index}", f"scala.Owner{index}", "method", str(index), "6", "A0", "ordinary", "literal")
            for index in range(5)
        ])
        with self.assertRaises(select_ordinary_holdout.HoldoutError):
            select_ordinary_holdout.select(self.inventory, self.main)


if __name__ == "__main__":
    unittest.main()
