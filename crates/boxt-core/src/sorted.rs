use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SortedVec<T: Ord>(Vec<T>);

impl<T: Ord> SortedVec<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn insert(&mut self, value: T) {
        let at = self.0.partition_point(|v| v <= &value);
        self.0.insert(at, value);
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T: Ord> Default for SortedVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord> From<Vec<T>> for SortedVec<T> {
    fn from(mut values: Vec<T>) -> Self {
        values.sort();
        Self(values)
    }
}

impl<T: Ord> FromIterator<T> for SortedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<T>>())
    }
}

impl<T: Ord> std::ops::Deref for SortedVec<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<'a, T: Ord> IntoIterator for &'a SortedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Ord + Serialize> Serialize for SortedVec<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Ord + Deserialize<'de>> Deserialize<'de> for SortedVec<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_deserialization_sorts() {
        let v: SortedVec<u32> = serde_json::from_str("[3,1,2,1]").unwrap();
        assert_eq!(v.as_slice(), &[1, 1, 2, 3]);
        assert_eq!(serde_json::to_string(&v).unwrap(), "[1,1,2,3]");
    }

    #[test]
    fn v3_insert_keeps_order() {
        let mut v = SortedVec::new();
        for x in [5, 1, 4, 1, 9, 0] {
            v.insert(x);
        }
        assert_eq!(v.as_slice(), &[0, 1, 1, 4, 5, 9]);
        let collected: SortedVec<i32> = [3, 2, 1].into_iter().collect();
        assert_eq!(collected.into_vec(), vec![1, 2, 3]);
    }
}
