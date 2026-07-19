//! Temporary efficiency benchmark; delete after review.

use std::time::Instant;

use hush::config::Config;
use hush::detect::{EntityKind, merge, rules::Rules};
use hush::pipeline;
use hush::vault::Vault;

fn dense_block() -> &'static str {
    "Bonjour Jean Dupont, votre commande est prete.\n\
        Contact: jean.dupont@example.com ou sales@sub.example.co.uk.\n\
        Appelez le 06 12 34 56 78 ou le +33 1 42 68 53 00 avant vendredi.\n\
        Virement sur FR14 2004 1010 0505 0001 3M02 606 svp.\n\
        Carte 4242 4242 4242 4242 enregistree le 12/03.\n\
        SIRET 12345678200002, SIREN 552100554.\n\
        Livraison au 12 bis rue de la Paix, 59000 Lille.\n\
        Serveur 192.168.1.10 et 2001:0db8:0000:0000:0000:ff00:0042:8329.\n\
        Reference interne 0000000 et 1234567 sans interet.\n\
        Le montant de 1234.56 euros est du au 2026-07-19 pour 9876543210 unites.\n\
        Texte de remplissage sans aucune entite particuliere ici presente.\n\n"
}

fn main() {
    let config = Config::default();
    let _ = Rules::new(&config).detect("Appelez le 06 12 34 56 78.");

    println!("== full clean(): detect + resolve + vault + replace_range ==");
    for repeats in [10, 100, 400] {
        let text = dense_block().repeat(repeats);
        let dir = tempfile::tempdir().expect("tempdir");
        let mut vault = Vault::open(&dir.path().join("v.db"), &dir.path().join("k")).expect("open");

        let spans = merge::resolve(Rules::new(&config).detect(&text));
        let span_count = spans.len();

        // Isolate the vault cost: every placeholder_for call the real clean makes.
        let start = Instant::now();
        for span in spans.iter().rev() {
            let _ = vault
                .placeholder_for(&span.kind, &span.text)
                .expect("placeholder");
        }
        let vault_time = start.elapsed();

        // Isolate the replace_range cost with placeholders already resolved.
        let placeholders: Vec<String> = spans
            .iter()
            .map(|s| {
                vault
                    .placeholder_for(&s.kind, &s.text)
                    .expect("placeholder")
            })
            .collect();
        let start = Instant::now();
        let mut output = text.clone();
        for (span, placeholder) in spans.iter().zip(&placeholders).rev() {
            output.replace_range(span.start..span.end, placeholder);
        }
        let splice_time = start.elapsed();

        // Single forward pass, for comparison.
        let start = Instant::now();
        let mut forward = String::with_capacity(text.len());
        let mut cursor = 0;
        for (span, placeholder) in spans.iter().zip(&placeholders) {
            forward.push_str(&text[cursor..span.start]);
            forward.push_str(placeholder);
            cursor = span.end;
        }
        forward.push_str(&text[cursor..]);
        let forward_time = start.elapsed();
        assert_eq!(output, forward, "both strategies must agree");

        let start = Instant::now();
        let mut fresh = Vault::open(&dir.path().join("v2.db"), &dir.path().join("k2")).unwrap();
        let _ = pipeline::clean(&text, &config, &mut fresh).expect("clean");
        let total = start.elapsed();

        println!(
            "bytes {:>8} spans {:>5}  vault(cold) {:>10.3?}  replace_range {:>10.3?}  forward {:>10.3?}  full clean {:>10.3?}",
            text.len(),
            span_count,
            vault_time,
            splice_time,
            forward_time,
            total
        );
    }

    println!("\n== repeated identical values: vault SELECT cost per occurrence ==");
    let dir = tempfile::tempdir().expect("tempdir");
    let mut vault = Vault::open(&dir.path().join("r.db"), &dir.path().join("rk")).expect("open");
    let _ = vault
        .placeholder_for(&EntityKind::Email, "a@example.com")
        .expect("seed");
    for n in [100, 1000, 10000] {
        let start = Instant::now();
        for _ in 0..n {
            let _ = vault
                .placeholder_for(&EntityKind::Email, "a@example.com")
                .expect("hit");
        }
        let elapsed = start.elapsed();
        println!(
            "{n:>6} repeat lookups: {elapsed:>10.3?}  ({:>8.3?} each)",
            elapsed / n
        );
    }
}
