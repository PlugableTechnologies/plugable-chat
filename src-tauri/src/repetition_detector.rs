/// Detects when a model is stuck in a repetition loop during streaming.
/// Triggers when: pattern_length * repetitions > 100 AND repetitions >= 3.
pub struct RepetitionDetector {
    buffer: String,
    max_buffer_size: usize,
    score_threshold: usize,
    min_repetitions: usize,
}

impl RepetitionDetector {
    /// Create a new repetition detector with default thresholds.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            max_buffer_size: 2000, // Increased from 1000 to catch longer loops
            score_threshold: 100,
            min_repetitions: 3,
        }
    }

    /// Add new text to the rolling buffer.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
        
        // Keep the buffer size within limits by removing old content from the start.
        if self.buffer.len() > self.max_buffer_size {
            let excess = self.buffer.len() - self.max_buffer_size;
            // Find the first valid char boundary to avoid panicking on multi-byte chars
            let mut start = excess;
            while start < self.buffer.len() && !self.buffer.is_char_boundary(start) {
                start += 1;
            }
            if start < self.buffer.len() {
                self.buffer = self.buffer[start..].to_string();
            }
        }
    }

    /// Returns (pattern, repetitions) if a loop is detected, None otherwise.
    pub fn detect_loop(&self) -> Option<(String, usize)> {
        // 1. Try exact match detection first (fastest)
        if let Some(res) = self.detect_in_string(&self.buffer) {
            return Some(res);
        }

        // 2. Try normalized detection (no whitespace, lowercase)
        // This catches loops where the model varies spacing or capitalization
        let normalized: String = self.buffer
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(|c| c.to_lowercase())
            .collect();
        
        if let Some((pattern, reps)) = self.detect_in_string(&normalized) {
            // Return a snippet of the normalized pattern to indicate what was found
            let preview = if pattern.len() > 50 {
                format!("{}...", &pattern[..47])
            } else {
                pattern
            };
            return Some((format!("{} (normalized)", preview), reps));
        }

        None
    }

    /// Core detection logic using period analysis.
    /// This is robust against trailing "noise" (partial repetitions at the end).
    fn detect_in_string(&self, s: &str) -> Option<(String, usize)> {
        let n = s.len();
        if n < self.min_repetitions {
            return None;
        }

        let bytes = s.as_bytes();
        
        // We look for a period L such that s[i] == s[i-L] for a significant stretch.
        // We try all possible periods L from 1 up to n/3.
        for l in 1..=(n / self.min_repetitions) {
            // Check if n-l is a valid char boundary to safely slice the pattern later
            if !s.is_char_boundary(n - l) {
                continue;
            }

            let mut matching_bytes = 0;
            // Count backwards how many bytes match their counterpart one period ago
            for i in (l..n).rev() {
                if bytes[i] == bytes[i - l] {
                    matching_bytes += 1;
                } else {
                    break;
                }
            }
            
            // Total repetitions = (matching stretch / period) + 1
            let reps = (matching_bytes / l) + 1;
            
            if reps >= self.min_repetitions {
                // Period pattern is the segment of length L at the end
                let pattern = &s[n - l..n];
                let pattern_char_len = pattern.chars().count();
                
                // Trigger based on total "meat" of the repetition
                if pattern_char_len * reps > self.score_threshold {
                    return Some((pattern.to_string(), reps));
                }
            }
        }
        
        None
    }
    
    /// Reset the detector's state.
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
}
