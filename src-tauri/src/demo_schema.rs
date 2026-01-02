//! Demo database schema definitions.
//!
//! This module contains the schema definition for the embedded Chicago Crimes
//! demo database, used for testing and demonstrating database functionality.

use crate::settings::{CachedColumnSchema, CachedTableSchema, SupportedDatabaseKind};

/// Source ID for the embedded demo database
pub const EMBEDDED_DEMO_SOURCE_ID: &str = "embedded-demo";

/// Display name for the embedded demo database
pub const EMBEDDED_DEMO_SOURCE_NAME: &str = "Chicago Crimes Demo";

/// Fully qualified table name for the Chicago crimes table
pub const CHICAGO_CRIMES_TABLE_FQ_NAME: &str = "main.chicago_crimes";

/// SQL DDL for creating the Chicago crimes table
pub const CHICAGO_CRIMES_CREATE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS chicago_crimes (
    id INTEGER PRIMARY KEY,
    case_number TEXT,
    date TEXT,
    block TEXT,
    iucr TEXT,
    primary_type TEXT,
    description TEXT,
    location_description TEXT,
    arrest INTEGER,
    domestic INTEGER,
    beat INTEGER,
    district INTEGER,
    ward REAL,
    community_area REAL,
    fbi_code TEXT,
    x_coordinate REAL,
    y_coordinate REAL,
    year INTEGER,
    updated_on TEXT,
    latitude REAL,
    longitude REAL,
    location TEXT,
    day_of_week TEXT,
    month_name TEXT,
    hour INTEGER,
    is_weekend INTEGER,
    season TEXT,
    is_business_hours INTEGER,
    community_area_name TEXT,
    hardship_index REAL,
    crime_category TEXT,
    location_zone TEXT,
    dist_from_center_km REAL,
    gun_involved INTEGER,
    child_involved INTEGER
)
"#;

/// CSV column headers expected in the Chicago crimes dataset
pub const CHICAGO_CRIMES_CSV_HEADERS: &[&str] = &[
    "ID",
    "Case Number",
    "Date",
    "Block",
    "IUCR",
    "Primary Type",
    "Description",
    "Location Description",
    "Arrest",
    "Domestic",
    "Beat",
    "District",
    "Ward",
    "Community Area",
    "FBI Code",
    "X Coordinate",
    "Y Coordinate",
    "Year",
    "Updated On",
    "Latitude",
    "Longitude",
    "Location",
    "Day_of_Week",
    "Month_Name",
    "Hour",
    "Is_Weekend",
    "Season",
    "Is_Business_Hours",
    "Community Area Name",
    "Hardship_Index",
    "Crime_Category",
    "Location_Zone",
    "Dist_From_Center_km",
    "Gun_Involved",
    "Child_Involved",
];

/// Get the cached table schema for the Chicago crimes table
pub fn chicago_crimes_table_schema() -> CachedTableSchema {
    CachedTableSchema {
        fully_qualified_name: CHICAGO_CRIMES_TABLE_FQ_NAME.to_string(),
        source_id: EMBEDDED_DEMO_SOURCE_ID.to_string(),
        kind: SupportedDatabaseKind::Sqlite,
        sql_dialect: "SQLite".to_string(),
        enabled: true,
        columns: chicago_crimes_columns(),
        primary_keys: vec!["id".to_string()],
        partition_columns: Vec::new(),
        cluster_columns: Vec::new(),
        description: Some(
            "Chicago crime incidents from 2025, enriched with geographic and temporal features"
                .to_string(),
        ),
    }
}

/// Get column metadata for the Chicago crimes table
pub fn chicago_crimes_columns() -> Vec<CachedColumnSchema> {
    vec![
        CachedColumnSchema {
            name: "id".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: false,
            description: Some("Unique identifier for the crime record".to_string()),
        },
        CachedColumnSchema {
            name: "case_number".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Chicago Police Department case number".to_string()),
        },
        CachedColumnSchema {
            name: "date".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Date and time when the incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "block".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Partially redacted address where incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "iucr".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Illinois Uniform Crime Reporting code".to_string()),
        },
        CachedColumnSchema {
            name: "primary_type".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Primary classification of the crime (e.g., THEFT, BATTERY)".to_string()),
        },
        CachedColumnSchema {
            name: "description".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Secondary description of the crime".to_string()),
        },
        CachedColumnSchema {
            name: "location_description".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Type of location where crime occurred (e.g., STREET, APARTMENT)".to_string()),
        },
        CachedColumnSchema {
            name: "arrest".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether an arrest was made (1=true, 0=false)".to_string()),
        },
        CachedColumnSchema {
            name: "domestic".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether the incident was domestic-related (1=true, 0=false)".to_string()),
        },
        CachedColumnSchema {
            name: "beat".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Police beat where incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "district".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Police district where incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "ward".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("City ward where incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "community_area".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Community area number where incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "fbi_code".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("FBI crime classification code".to_string()),
        },
        CachedColumnSchema {
            name: "x_coordinate".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("State Plane X coordinate".to_string()),
        },
        CachedColumnSchema {
            name: "y_coordinate".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("State Plane Y coordinate".to_string()),
        },
        CachedColumnSchema {
            name: "year".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Year the incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "updated_on".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Date and time the record was last updated".to_string()),
        },
        CachedColumnSchema {
            name: "latitude".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Latitude of the incident location".to_string()),
        },
        CachedColumnSchema {
            name: "longitude".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Longitude of the incident location".to_string()),
        },
        CachedColumnSchema {
            name: "location".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Combined latitude/longitude as text".to_string()),
        },
        CachedColumnSchema {
            name: "day_of_week".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Day of week when incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "month_name".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Month name when incident occurred".to_string()),
        },
        CachedColumnSchema {
            name: "hour".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Hour of day when incident occurred (0-23)".to_string()),
        },
        CachedColumnSchema {
            name: "is_weekend".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether incident occurred on weekend (1=true, 0=false)".to_string()),
        },
        CachedColumnSchema {
            name: "season".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Season when incident occurred (Winter, Spring, Summer, Fall)".to_string()),
        },
        CachedColumnSchema {
            name: "is_business_hours".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether incident occurred during business hours (1=true, 0=false)".to_string()),
        },
        CachedColumnSchema {
            name: "community_area_name".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Name of the community area".to_string()),
        },
        CachedColumnSchema {
            name: "hardship_index".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Socioeconomic hardship index for the community area".to_string()),
        },
        CachedColumnSchema {
            name: "crime_category".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("High-level crime category (Violent, Property, etc.)".to_string()),
        },
        CachedColumnSchema {
            name: "location_zone".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Type of location zone (Public Open, Private Restricted, etc.)".to_string()),
        },
        CachedColumnSchema {
            name: "dist_from_center_km".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Distance from Chicago city center in kilometers".to_string()),
        },
        CachedColumnSchema {
            name: "gun_involved".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether a gun was involved (1=true, 0=false)".to_string()),
        },
        CachedColumnSchema {
            name: "child_involved".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether a child was involved (1=true, 0=false)".to_string()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chicago_crimes_schema_has_correct_columns() {
        let schema = chicago_crimes_table_schema();
        assert_eq!(schema.columns.len(), 35);
        assert_eq!(schema.fully_qualified_name, "main.chicago_crimes");
        assert_eq!(schema.source_id, "embedded-demo");
    }

    #[test]
    fn test_csv_headers_match_columns() {
        let schema = chicago_crimes_table_schema();
        assert_eq!(CHICAGO_CRIMES_CSV_HEADERS.len(), schema.columns.len());
    }
}
