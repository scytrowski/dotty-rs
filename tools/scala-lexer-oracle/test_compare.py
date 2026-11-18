import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import compare


class NormalizationTests(unittest.TestCase):
    def test_classifies_nested_string_from_scanner_context_after_non_ascii(self):
        source = '𐐀 s"${"x"}"'
        lines = [
            "identifier\t0\t3\t0\t𐐀\t\t0\tidentifier",
            "string interpolator\t3\t4\t0\ts\t\t0\t",
            "string literal\t4\t6\t0\t\t\t0\tstring part",
            "'{'\t6\t7\t0\t\t\t0\t",
            "string literal\t7\t10\t0\t\tx\t0\t",
            "'}'\t10\t11\t0\t\t\t0\t",
            "string literal\t11\t12\t0\t\t\t0\tstring part",
            "eof\t12\t12\t0\t\t\t0\t",
        ]
        self.assertEqual(compare.normalize_oracle(lines, source), [
            ("identifier", 0), ("string interpolator", 5), ("string part", 6),
            ("'{'", 8), ("string literal", 9), ("'}'", 12),
            ("string part", 13), ("eof", 14),
        ])

    def test_maps_wildcard_oracle_token_to_the_shared_identifier_surface(self):
        self.assertEqual(compare.oracle_kind("_"), "identifier")

    def test_maps_parser_colon_protocol_to_an_operator_for_raw_comparison(self):
        self.assertEqual(
            compare.normalize_rust(
                ["kind\tstart\tend", "':'\t8\t9"]
            ),
            [("operator", 8)],
        )


if __name__ == "__main__":
    unittest.main()
