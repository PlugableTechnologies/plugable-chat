//! Demo database schema definitions.
//!
//! This module contains the schema definition for the embedded Chicago Crimes
//! demo database, used for testing and demonstrating database functionality.

use crate::settings::{CachedColumnSchema, CachedTableSchema, SupportedDatabaseKind};

/// Schema version for the embedded demo database.
/// Bump this when the schema changes to trigger automatic migration.
pub const DEMO_SCHEMA_VERSION: i32 = 1;

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
    date_of_crime TEXT,
    time_of_crime TEXT,
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
    "Date_of_Crime",
    "Time_of_Crime",
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
            special_attributes: vec!["primary_key".to_string()],
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "case_number".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Chicago Police Department case number".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "date_of_crime".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Date when the incident occurred (YYYY-MM-DD format)".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "time_of_crime".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Time when the incident occurred (HH:MM:SS 24-hour format)".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "block".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Partially redacted address where incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "iucr".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Illinois Uniform Crime Reporting code".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "primary_type".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Primary classification of the crime (e.g., THEFT, BATTERY)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["THEFT (18.5%)".to_string(), "BATTERY (15.2%)".to_string(), "CRIMINAL DAMAGE (9.8%)".to_string()],
        },
        CachedColumnSchema {
            name: "description".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Secondary description of the crime".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "location_description".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Type of location where crime occurred (e.g., STREET, APARTMENT)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["STREET (22.1%)".to_string(), "APARTMENT (16.3%)".to_string(), "RESIDENCE (12.5%)".to_string()],
        },
        CachedColumnSchema {
            name: "arrest".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether an arrest was made (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (82.1%)".to_string(), "1 (17.9%)".to_string()],
        },
        CachedColumnSchema {
            name: "domestic".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether the incident was domestic-related (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (84.5%)".to_string(), "1 (15.5%)".to_string()],
        },
        CachedColumnSchema {
            name: "beat".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Police beat where incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "district".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Police district where incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "ward".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("City ward where incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "community_area".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Community area number where incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "fbi_code".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("FBI crime classification code".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["06 (25.3%)".to_string(), "08B (15.1%)".to_string(), "14 (10.2%)".to_string()],
        },
        CachedColumnSchema {
            name: "x_coordinate".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("State Plane X coordinate".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "y_coordinate".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("State Plane Y coordinate".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "year".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Year the incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["2025 (100.0%)".to_string()],
        },
        CachedColumnSchema {
            name: "latitude".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Latitude of the incident location".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "longitude".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Longitude of the incident location".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "location".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Combined latitude/longitude as text".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "day_of_week".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Day of week when incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["Friday (15.2%)".to_string(), "Saturday (14.8%)".to_string(), "Wednesday (14.5%)".to_string()],
        },
        CachedColumnSchema {
            name: "month_name".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Month name when incident occurred".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "hour".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Hour of day when incident occurred (0-23)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["12 (5.8%)".to_string(), "0 (5.5%)".to_string(), "18 (5.3%)".to_string()],
        },
        CachedColumnSchema {
            name: "is_weekend".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether incident occurred on weekend (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (71.4%)".to_string(), "1 (28.6%)".to_string()],
        },
        CachedColumnSchema {
            name: "season".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Season when incident occurred (Winter, Spring, Summer, Fall)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["Winter (35.2%)".to_string(), "Fall (25.1%)".to_string(), "Summer (20.5%)".to_string()],
        },
        CachedColumnSchema {
            name: "is_business_hours".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether incident occurred during business hours (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (66.8%)".to_string(), "1 (33.2%)".to_string()],
        },
        CachedColumnSchema {
            name: "community_area_name".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Name of the community area".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["Austin (5.8%)".to_string(), "Near North Side (4.2%)".to_string(), "Loop (3.9%)".to_string()],
        },
        CachedColumnSchema {
            name: "hardship_index".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Socioeconomic hardship index for the community area".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "crime_category".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("High-level crime category (Violent, Property, etc.)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["Property (42.5%)".to_string(), "Violent (28.3%)".to_string(), "Other (15.2%)".to_string()],
        },
        CachedColumnSchema {
            name: "location_zone".to_string(),
            data_type: "TEXT".to_string(),
            nullable: true,
            description: Some("Type of location zone (Public Open, Private Restricted, etc.)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["Public Open (35.2%)".to_string(), "Private Restricted (28.5%)".to_string(), "Commercial (18.1%)".to_string()],
        },
        CachedColumnSchema {
            name: "dist_from_center_km".to_string(),
            data_type: "REAL".to_string(),
            nullable: true,
            description: Some("Distance from Chicago city center in kilometers".to_string()),
            special_attributes: Vec::new(),
            top_values: Vec::new(),
        },
        CachedColumnSchema {
            name: "gun_involved".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether a gun was involved (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (95.2%)".to_string(), "1 (4.8%)".to_string()],
        },
        CachedColumnSchema {
            name: "child_involved".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: true,
            description: Some("Whether a child was involved (1=true, 0=false)".to_string()),
            special_attributes: Vec::new(),
            top_values: vec!["0 (98.5%)".to_string(), "1 (1.5%)".to_string()],
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
