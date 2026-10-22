use super::{ArgumentKind, ArgumentSpec, CommandSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Literal(&'static str),
    Argument(ArgumentSpec),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub children: Vec<usize>,
    pub executable: bool,
}

/// Transport-neutral graph. Each child precedes its parent, and choice branches
/// share their argument suffix rather than expanding a Cartesian product.
#[derive(Debug, PartialEq, Eq)]
pub struct CommandTree {
    pub nodes: Vec<Node>,
    pub root: usize,
}

impl CommandTree {
    pub fn new(commands: &[CommandSpec]) -> Self {
        let mut tree = Self { nodes: Vec::new(), root: 0 };
        let mut children = Vec::new();
        for command in commands {
            let mut forms = Vec::new();
            let mut can_execute = false;
            command.syntax.sequences(&mut |arguments| {
                let mut suffix = Vec::new();
                let mut executable = true;
                for &argument in arguments.iter().rev() {
                    let kinds: Vec<_> = match argument.kind {
                        ArgumentKind::Choices(values) => {
                            values.iter().map(|value| NodeKind::Literal(value)).collect()
                        }
                        _ => vec![NodeKind::Argument(argument)],
                    };
                    let mut branches = Vec::new();
                    for kind in kinds {
                        branches.push(tree.nodes.len());
                        tree.nodes.push(Node { kind, children: suffix.clone(), executable });
                    }
                    suffix = branches;
                    executable &= argument.optional;
                }
                can_execute |= executable;
                tree.merge(&mut forms, suffix);
            });
            children.push(tree.nodes.len());
            tree.nodes.push(Node {
                kind: NodeKind::Literal(command.name),
                children: forms,
                executable: can_execute,
            });
        }
        tree.root = tree.nodes.len();
        tree.nodes.push(Node { kind: NodeKind::Root, children, executable: false });
        let mut ordered = Self { nodes: Vec::new(), root: 0 };
        let mut indices = vec![None; tree.nodes.len()];
        ordered.root = Self::emit(tree.root, &tree.nodes, &mut indices, &mut ordered.nodes);
        ordered
    }

    fn merge(&mut self, roots: &mut Vec<usize>, incoming: Vec<usize>) {
        for id in incoming {
            if let Some(&existing) =
                roots.iter().find(|&&old| self.nodes[old].kind == self.nodes[id].kind)
            {
                self.nodes[existing].executable |= self.nodes[id].executable;
                let mut children = std::mem::take(&mut self.nodes[existing].children);
                self.merge(&mut children, self.nodes[id].children.clone());
                self.nodes[existing].children = children;
            } else {
                roots.push(id);
            }
        }
    }

    fn emit(
        id: usize,
        source: &[Node],
        indices: &mut [Option<usize>],
        output: &mut Vec<Node>,
    ) -> usize {
        if let Some(index) = indices[id] {
            return index;
        }
        let children = source[id]
            .children
            .iter()
            .map(|&child| Self::emit(child, source, indices, output))
            .collect();
        let index = output.len();
        output.push(Node { kind: source[id].kind, children, executable: source[id].executable });
        indices[id] = Some(index);
        index
    }
}
