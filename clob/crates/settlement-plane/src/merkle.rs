use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

/// A Merkle tree implementation.
#[derive(Debug)]
pub struct MerkleTree {
    nodes: Vec<Hash>,
}

impl MerkleTree {
    /// Constructs a new Merkle tree from a vector of leaf hashes.
    pub fn new(leaves: &[Hash]) -> Self {
        if leaves.is_empty() {
            return MerkleTree { nodes: vec![[0; 32]] };
        }

        let mut nodes = Vec::new();
        let mut current_level = leaves.to_vec();

        // Add the leaf level to the nodes vector
        nodes.extend_from_slice(&current_level);

        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            // Process nodes in pairs
            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                // If there's an odd number of nodes, duplicate the last one
                let right = if chunk.len() > 1 { chunk[1] } else { left };
                
                let mut hasher = Sha256::new();
                hasher.update(&left);
                hasher.update(&right);
                let parent_hash: Hash = hasher.finalize().into();
                next_level.push(parent_hash);
            }
            current_level = next_level;
            nodes.extend_from_slice(&current_level);
        }
        
        MerkleTree { nodes }
    }

    /// Returns the root hash of the Merkle tree.
    pub fn root(&self) -> Hash {
        self.nodes.last().cloned().unwrap_or([0; 32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_string(s: &str) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(s.as_bytes());
        hasher.finalize().into()
    }

    #[test]
    fn test_merkle_tree_even_leaves() {
        let leaves = vec![
            hash_string("a"),
            hash_string("b"),
            hash_string("c"),
            hash_string("d"),
        ];
        let tree = MerkleTree::new(&leaves);

        let ab = hash_string("ab"); // Not quite, need to hash the hashes
        let mut hasher_ab = Sha256::new();
        hasher_ab.update(&leaves[0]);
        hasher_ab.update(&leaves[1]);
        let hash_ab: Hash = hasher_ab.finalize().into();

        let mut hasher_cd = Sha256::new();
        hasher_cd.update(&leaves[2]);
        hasher_cd.update(&leaves[3]);
        let hash_cd: Hash = hasher_cd.finalize().into();

        let mut hasher_abcd = Sha256::new();
        hasher_abcd.update(&hash_ab);
        hasher_abcd.update(&hash_cd);
        let expected_root: Hash = hasher_abcd.finalize().into();

        assert_eq!(tree.root(), expected_root);
    }

    #[test]
    fn test_merkle_tree_odd_leaves() {
        let leaves = vec![
            hash_string("a"),
            hash_string("b"),
            hash_string("c"),
        ];
        let tree = MerkleTree::new(&leaves);

        let mut hasher_ab = Sha256::new();
        hasher_ab.update(&leaves[0]);
        hasher_ab.update(&leaves[1]);
        let hash_ab: Hash = hasher_ab.finalize().into();

        // The odd leaf 'c' is paired with itself
        let mut hasher_cc = Sha256::new();
        hasher_cc.update(&leaves[2]);
        hasher_cc.update(&leaves[2]);
        let hash_cc: Hash = hasher_cc.finalize().into();

        let mut hasher_abcc = Sha256::new();
        hasher_abcc.update(&hash_ab);
        hasher_abcc.update(&hash_cc);
        let expected_root: Hash = hasher_abcc.finalize().into();

        assert_eq!(tree.root(), expected_root);
    }
}