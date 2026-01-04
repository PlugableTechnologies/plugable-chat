#!/usr/bin/env python3
"""
Transform Chicago Crimes CSV to fix date column confusion.

This script:
- Renames "Date" → "Date_of_Crime" (YYYY-MM-DD format)
- Adds "Time_of_Crime" column (HH:MM:SS 24-hour format)
- Removes "Updated On" column
"""

import csv
from datetime import datetime
from pathlib import Path


def transform_date(date_str: str) -> tuple[str, str]:
    """
    Parse datetime string and split into date and time components.
    
    Input format: "MM/DD/YYYY HH:MM:SS AM/PM" (e.g., "01/01/2025 03:57:00 AM")
    Output: ("YYYY-MM-DD", "HH:MM:SS")
    """
    dt = datetime.strptime(date_str, "%m/%d/%Y %I:%M:%S %p")
    return dt.strftime("%Y-%m-%d"), dt.strftime("%H:%M:%S")


def transform_csv(input_path: Path, output_path: Path) -> int:
    """
    Transform the CSV file with new date columns.
    
    Returns the number of rows processed.
    """
    with open(input_path, 'r', newline='', encoding='utf-8') as infile:
        reader = csv.DictReader(infile)
        
        # Build new fieldnames
        old_fieldnames = reader.fieldnames
        if not old_fieldnames:
            raise ValueError("CSV has no headers")
        
        # Find the index of "Date" to insert new columns in same position
        date_idx = old_fieldnames.index("Date")
        
        # Build new fieldnames:
        # - Replace "Date" with "Date_of_Crime" 
        # - Add "Time_of_Crime" right after
        # - Remove "Updated On"
        new_fieldnames = []
        for i, field in enumerate(old_fieldnames):
            if field == "Date":
                new_fieldnames.append("Date_of_Crime")
                new_fieldnames.append("Time_of_Crime")
            elif field == "Updated On":
                continue  # Skip this column
            else:
                new_fieldnames.append(field)
        
        print(f"Old columns ({len(old_fieldnames)}): {old_fieldnames}")
        print(f"New columns ({len(new_fieldnames)}): {new_fieldnames}")
        
        # Process rows
        rows = []
        row_count = 0
        for row in reader:
            # Transform date
            date_of_crime, time_of_crime = transform_date(row["Date"])
            
            # Build new row
            new_row = {}
            for field in new_fieldnames:
                if field == "Date_of_Crime":
                    new_row[field] = date_of_crime
                elif field == "Time_of_Crime":
                    new_row[field] = time_of_crime
                else:
                    new_row[field] = row[field]
            
            rows.append(new_row)
            row_count += 1
            
            if row_count % 5000 == 0:
                print(f"Processed {row_count} rows...")
        
        print(f"Total rows processed: {row_count}")
    
    # Write output
    with open(output_path, 'w', newline='', encoding='utf-8') as outfile:
        writer = csv.DictWriter(outfile, fieldnames=new_fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    
    print(f"Written to: {output_path}")
    return row_count


def main():
    # Find test-data directory
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    test_data_dir = project_root / "test-data"
    
    csv_path = test_data_dir / "Chicago_Crimes_2025_Enriched.csv"
    
    if not csv_path.exists():
        print(f"ERROR: CSV not found at {csv_path}")
        return 1
    
    # Create backup
    backup_path = test_data_dir / "Chicago_Crimes_2025_Enriched.csv.backup"
    if not backup_path.exists():
        import shutil
        shutil.copy(csv_path, backup_path)
        print(f"Created backup at: {backup_path}")
    
    # Transform in place
    temp_path = test_data_dir / "Chicago_Crimes_2025_Enriched.csv.tmp"
    
    try:
        row_count = transform_csv(csv_path, temp_path)
        
        # Replace original with transformed
        temp_path.replace(csv_path)
        print(f"Successfully transformed {row_count} rows")
        return 0
        
    except Exception as e:
        print(f"ERROR: {e}")
        if temp_path.exists():
            temp_path.unlink()
        return 1


if __name__ == "__main__":
    exit(main())
