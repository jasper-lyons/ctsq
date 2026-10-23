mod cache;
mod util;

use cache::Cache;
use std::collections::HashMap;

// an outside receiver type: HashMap::insert, not Cache::insert
pub fn outside() {
    let mut m = HashMap::new();
    m.insert(1, 2);
}

// a bare call to a function no import brings into scope
pub fn bare() {
    helper();
}

// positive control: a typed receiver
pub fn control(c: &Cache) {
    c.insert(1, 2);
}

// positive control: an imported path
pub fn imported() {
    cache::make();
}
