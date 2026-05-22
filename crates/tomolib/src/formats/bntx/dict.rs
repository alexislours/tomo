use crate::Result;
use crate::formats::bntx::Reader;

const DIC_MAGIC: [u8; 4] = *b"_DIC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub reference: u32,
    pub left: u16,
    pub right: u16,
    pub key: Option<usize>,
}

const ROOT_REF: u32 = 0xFFFF_FFFF;

pub(crate) fn entry_count(r: &Reader, off: usize) -> Result<usize> {
    if crate::formats::binio::read_array::<4>(r.bytes, off, "_DIC magic")? != DIC_MAGIC {
        return Err(crate::Error::malformed(
            "BNTX: dictionary missing _DIC magic",
        ));
    }
    Ok(r.u32_at(off + 4)? as usize)
}

#[inline]
fn get_bit(key: &[u8], bit: u32) -> u8 {
    if bit == ROOT_REF {
        return 0;
    }
    let idx = (bit >> 3) as usize;
    if idx >= key.len() {
        return 0;
    }
    (key[key.len() - 1 - idx] >> (bit & 7)) & 1
}

fn first_mismatch(a: &[u8], b: &[u8]) -> Option<u32> {
    let bits = 8 * u32::try_from(a.len().max(b.len())).unwrap_or(u32::MAX / 8);
    (0..bits).find(|&i| get_bit(a, i) != get_bit(b, i))
}

pub(crate) fn build(keys: &[String]) -> Vec<Node> {
    let mut nodes = vec![Node {
        reference: ROOT_REF,
        left: 0,
        right: 0,
        key: None,
    }];

    for (ki, key) in keys.iter().enumerate() {
        let kb = key.as_bytes();
        let new_index = u16::try_from(nodes.len()).expect("dictionary entry count fits in u16");

        let leaf = descend(&nodes, kb);
        let leaf_key = node_key(&nodes[leaf as usize], keys);
        let bit_idx = first_mismatch(leaf_key, kb)
            .unwrap_or_else(|| 8 * u32::try_from(kb.len()).unwrap_or(0));

        let bit_ref = signed(bit_idx);
        let mut parent = 0u16;
        let mut parent_right = false;
        let mut child = nodes[0].left;
        while signed(nodes[child as usize].reference) < bit_ref
            && signed(nodes[child as usize].reference) > signed(nodes[parent as usize].reference)
        {
            parent = child;
            parent_right = get_bit(kb, nodes[child as usize].reference) == 1;
            child = step(&nodes[child as usize], kb);
        }

        let bit = get_bit(kb, bit_idx);
        let mut node = Node {
            reference: bit_idx,
            left: 0,
            right: 0,
            key: Some(ki),
        };
        if bit == 1 {
            node.right = new_index;
            node.left = child;
        } else {
            node.left = new_index;
            node.right = child;
        }
        nodes.push(node);

        if parent_right {
            nodes[parent as usize].right = new_index;
        } else {
            nodes[parent as usize].left = new_index;
        }
    }

    nodes
}

fn node_key<'a>(node: &Node, keys: &'a [String]) -> &'a [u8] {
    node.key.map_or(&[][..], |i| keys[i].as_bytes())
}

#[inline]
fn signed(reference: u32) -> i32 {
    reference.cast_signed()
}

fn step(node: &Node, key: &[u8]) -> u16 {
    if get_bit(key, node.reference) == 1 {
        node.right
    } else {
        node.left
    }
}

fn descend(nodes: &[Node], key: &[u8]) -> u16 {
    let mut prev_ref = signed(nodes[0].reference);
    let mut idx = nodes[0].left;
    loop {
        let r = signed(nodes[idx as usize].reference);
        if r <= prev_ref {
            return idx;
        }
        prev_ref = r;
        idx = step(&nodes[idx as usize], key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_key_matches_nintendo_layout() {
        let keys = vec!["Face_Mouth035".to_string()];
        let nodes = build(&keys);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].reference, ROOT_REF);
        assert_eq!((nodes[0].left, nodes[0].right), (1, 0));
        assert_eq!(nodes[1].reference, 0);
        assert_eq!((nodes[1].left, nodes[1].right), (0, 1));
    }

    #[test]
    fn lookups_are_self_consistent() {
        let keys: Vec<String> = [
            "alpha",
            "beta",
            "gamma",
            "delta",
            "MiiFaceline00_Pos",
            "a",
            "ab",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let nodes = build(&keys);
        for (ki, k) in keys.iter().enumerate() {
            let idx = descend(&nodes, k.as_bytes());
            assert_eq!(
                nodes[idx as usize].key,
                Some(ki),
                "lookup of {k} landed on the wrong node"
            );
        }
    }
}
