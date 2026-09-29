use syn::{Attribute, Expr, ExprLit, Lit, Meta};

pub enum Format {
    Standard,
    VarInt,
    Json,
}

impl From<&str> for Format {
    fn from(s: &str) -> Self {
        match s {
            "varint" => Format::VarInt,
            "json" => Format::Json,
            _ => Format::Standard,
        }
    }
}

impl From<&Attribute> for Format {
    fn from(attr: &Attribute) -> Format {
        let Meta::NameValue(nv) = &attr.meta else {
            return Format::Standard;
        };

        if !nv.path.is_ident("format") {
            return Format::Standard;
        }

        if let Expr::Lit(ExprLit { attrs: _, lit: Lit::Str(s) }) = &nv.value {
            return Format::from(s.value().as_str());
        };

        Format::Standard
    }
}

impl From<&Vec<Attribute>> for Format {
    fn from(attrs: &Vec<Attribute>) -> Format {
        attrs.iter().map(Format::from).nth(0).unwrap_or(Format::Standard)
    }
}
