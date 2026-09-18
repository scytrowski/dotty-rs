import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import compare


class NormalizationTests(unittest.TestCase):
    def test_ignores_interpolator_shaped_text_inside_an_ordinary_string(self):
        source = 'case "string interpolator"'

        self.assertEqual(compare.find_interpolation_ranges(source), [])

    def test_finds_the_outer_end_after_quotes_inside_a_braced_splice(self):
        source = 's"${missing.mkString(", ")}"'

        self.assertEqual(
            compare.find_interpolation_ranges(source), [(1, len(source.encode()))]
        )

    def test_does_not_mark_a_string_inside_a_braced_splice_as_string_part(self):
        source = 's"${missing.mkString(", ")}"'
        inner_quote = source.index('"', source.index("mkString") + len("mkString"))

        self.assertTrue(
            compare.is_in_interpolation_expression(source, 1, inner_quote)
        )

    def test_maps_wildcard_oracle_token_to_the_shared_identifier_surface(self):
        self.assertEqual(compare.oracle_kind("_", "_", "_", False), "identifier")

    def test_maps_parser_colon_protocol_to_an_operator_for_raw_comparison(self):
        self.assertEqual(
            compare.normalize_rust(["kind\tstart\tend", "':'\t8\t9"]),
            [("operator", 8)],
        )

    def test_marks_an_interpolated_string_body_as_string_part(self):
        source = 's"hello $name"'
        lines = [
            "interpolation id\t0\t1\t0\ts\t\t",
            "string literal\t1\t14\t0\t\t\t",
        ]

        self.assertEqual(
            compare.normalize_oracle(lines, source),
            [("interpolation id", 0), ("string part", 1)],
        )


if __name__ == "__main__":
    unittest.main()
