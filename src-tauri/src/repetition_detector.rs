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
            max_buffer_size: 1000,
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
        let buf = &self.buffer;
        let buf_len = buf.len();
        
        if buf_len < self.min_repetitions {
            return None;
        }

        // Try pattern lengths from 1 up to buf_len / min_reps.
        // We start from 1 to catch single character repetition quickly.
        for pattern_len in 1..=(buf_len / self.min_repetitions) {
            let pattern_end = buf_len;
            let pattern_start = buf_len - pattern_len;
            
            // Check if pattern_start is a valid char boundary
            if !buf.is_char_boundary(pattern_start) {
                continue;
            }
            
            let pattern = &buf[pattern_start..pattern_end];
            let mut reps = 1;
            let mut pos = pattern_start;
            
            // Count consecutive occurrences backwards.
            while pos >= pattern_len {
                let prev_pos = pos - pattern_len;
                if !buf.is_char_boundary(prev_pos) {
                    break;
                }
                
                if &buf[prev_pos..pos] == pattern {
                    reps += 1;
                    pos = prev_pos;
                } else {
                    break;
                }
            }
            
            // Formula: pattern_length * repetitions > score_threshold AND repetitions >= min_repetitions
            // Using chars().count() for length to be more accurate with multi-byte chars, 
            // though byte length is usually fine for these thresholds.
            let pattern_char_len = pattern.chars().count();
            if reps >= self.min_repetitions && pattern_char_len * reps > self.score_threshold {
                return Some((pattern.to_string(), reps));
            }
        }
        None
    }
    
    /// Reset the detector's state.
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
}
