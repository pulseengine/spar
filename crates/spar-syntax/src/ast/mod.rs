use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Trait for typed AST nodes wrapping a [`SyntaxNode`].
pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

// Helper functions for AST accessors.

fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

fn children<'a, N: AstNode + 'a>(parent: &'a SyntaxNode) -> impl Iterator<Item = N> + 'a {
    parent.children().filter_map(N::cast)
}

fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|it| it.kind() == kind)
}

// === Typed AST Nodes ===

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone)]
        pub struct $name {
            syntax: SyntaxNode,
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }
            fn cast(node: SyntaxNode) -> Option<Self> {
                if Self::can_cast(node.kind()) {
                    Some(Self { syntax: node })
                } else {
                    None
                }
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.syntax
            }
        }
    };
}

ast_node!(SourceFile, SOURCE_FILE);
ast_node!(AadlPackage, AADL_PACKAGE);
ast_node!(PublicSection, PUBLIC_SECTION);
ast_node!(PrivateSection, PRIVATE_SECTION);
ast_node!(WithClause, WITH_CLAUSE);
ast_node!(ComponentType, COMPONENT_TYPE);
ast_node!(ComponentImpl, COMPONENT_IMPL);
ast_node!(ComponentCategory, COMPONENT_CATEGORY);
ast_node!(FeatureSection, FEATURE_SECTION);
ast_node!(DataPort, DATA_PORT);
ast_node!(EventPort, EVENT_PORT);
ast_node!(EventDataPort, EVENT_DATA_PORT);
ast_node!(SubcomponentSection, SUBCOMPONENT_SECTION);
ast_node!(Subcomponent, SUBCOMPONENT);
ast_node!(ConnectionSection, CONNECTION_SECTION);
ast_node!(FlowSpec, FLOW_SPEC);
ast_node!(PropertyAssociation, PROPERTY_ASSOCIATION);
ast_node!(AnnexSubclause, ANNEX_SUBCLAUSE);
ast_node!(PropertySet, PROPERTY_SET);
ast_node!(FeatureGroupType, FEATURE_GROUP_TYPE);

// === Accessors ===

impl SourceFile {
    pub fn packages(&self) -> impl Iterator<Item = AadlPackage> + '_ {
        children(&self.syntax)
    }

    pub fn property_sets(&self) -> impl Iterator<Item = PropertySet> + '_ {
        children(&self.syntax)
    }
}

impl AadlPackage {
    pub fn name_token(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::IDENT)
    }

    pub fn public_section(&self) -> Option<PublicSection> {
        child(&self.syntax)
    }

    pub fn private_section(&self) -> Option<PrivateSection> {
        child(&self.syntax)
    }
}

impl ComponentType {
    pub fn category(&self) -> Option<ComponentCategory> {
        child(&self.syntax)
    }

    pub fn name_token(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::IDENT)
    }

    pub fn feature_section(&self) -> Option<FeatureSection> {
        child(&self.syntax)
    }
}

impl ComponentImpl {
    pub fn category(&self) -> Option<ComponentCategory> {
        child(&self.syntax)
    }

    pub fn subcomponent_section(&self) -> Option<SubcomponentSection> {
        child(&self.syntax)
    }

    pub fn connection_section(&self) -> Option<ConnectionSection> {
        child(&self.syntax)
    }
}
