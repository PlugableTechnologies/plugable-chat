"""
Export schema cache (schema_tables and schema_columns) from LanceDB to TSV files
in a temporary directory, printing full file paths.

Requires: lancedb (pip install lancedb)
"""

from pathlib import Path
import sys
import tempfile

try:
    from lancedb import connect
except ImportError:
    print("ERROR: lancedb is not installed. Install with `pip install lancedb`.", file=sys.stderr)
    sys.exit(1)


def main() -> int:
    base = Path(__file__).resolve().parents[1] / "src-tauri" / "data" / "lancedb"
    if not base.exists():
        print(f"ERROR: LanceDB path does not exist: {base}", file=sys.stderr)
        return 1

    db = connect(base)
    tables = {}
    for name in ("schema_tables", "schema_columns"):
        if name not in db.table_names():
            print(f"WARNING: Missing table {name} in {base}", file=sys.stderr)
            continue
        tables[name] = db.open_table(name).to_pandas()

    if not tables:
        print("No tables exported (none found).", file=sys.stderr)
        return 1

    out_dir = Path(tempfile.mkdtemp(prefix="schema_cache_tsv_"))
    for name, df in tables.items():
        out_path = out_dir / f"{name}.tsv"
        df.to_csv(out_path, sep="\t", index=False)
        print(out_path)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
