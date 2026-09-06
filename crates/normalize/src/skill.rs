//! The skill taxonomy.
//!
//! Two jobs here. First, fold the many spellings of a technology onto one node, so
//! `"K8s"`, `"Kubernetes"` and `"kubernetes (EKS)"` are the same skill. Second, record a
//! *hierarchy*, so React experience can earn partial credit against a JavaScript
//! requirement — but never the reverse (`docs/06-extraction.md` §5).
//!
//! The seed set below covers the software market this tool was built for. It is a starting
//! point, not a closed vocabulary: unrecognized phrases become `skill_candidate` rows for
//! review rather than being dropped, so the taxonomy grows to fit the user's field.

use jobseeker_core::domain::requirement::SkillKind;
use jobseeker_core::slug::slugify;
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// A taxonomy entry before it gets a database id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedSkill {
    pub slug: &'static str,
    pub name: &'static str,
    pub kind: SkillKind,
    /// Slug of the parent node, if any. Evidence flows child → parent, never parent → child.
    pub parent: Option<&'static str>,
    /// Spellings that resolve to this skill. Matched case- and punctuation-insensitively.
    pub aliases: &'static [&'static str],
}

macro_rules! skill {
    ($slug:literal, $name:literal, $kind:ident $(, parent = $parent:literal)? $(, aliases = [$($alias:literal),* $(,)?])? $(,)?) => {
        SeedSkill {
            slug: $slug,
            name: $name,
            kind: SkillKind::$kind,
            parent: {
                #[allow(unused_assignments, unused_mut)]
                let mut p: Option<&'static str> = None;
                $( p = Some($parent); )?
                p
            },
            aliases: &[ $( $($alias),* )? ],
        }
    };
}

/// The seed taxonomy. Parents are declared before children so a loader can insert in order.
pub static SEED_SKILLS: Lazy<Vec<SeedSkill>> = Lazy::new(|| {
    vec![
        // ---- languages ---------------------------------------------------------------
        skill!("programming", "Programming", Concept),
        skill!(
            "rust",
            "Rust",
            Language,
            parent = "programming",
            aliases = ["rustlang", "rust-lang"]
        ),
        skill!("c", "C", Language, parent = "programming"),
        skill!(
            "cpp",
            "C++",
            Language,
            parent = "programming",
            aliases = ["c++", "cplusplus", "c/c++"]
        ),
        skill!(
            "csharp",
            "C#",
            Language,
            parent = "programming",
            aliases = ["c#", "c sharp", "dotnet c#"]
        ),
        skill!(
            "go",
            "Go",
            Language,
            parent = "programming",
            aliases = ["golang"]
        ),
        skill!("java", "Java", Language, parent = "programming"),
        skill!("kotlin", "Kotlin", Language, parent = "java"),
        skill!("scala", "Scala", Language, parent = "java"),
        skill!(
            "javascript",
            "JavaScript",
            Language,
            parent = "programming",
            aliases = ["js", "es6", "ecmascript", "vanilla js"]
        ),
        skill!(
            "typescript",
            "TypeScript",
            Language,
            parent = "javascript",
            aliases = ["ts"]
        ),
        skill!(
            "python",
            "Python",
            Language,
            parent = "programming",
            aliases = ["python3", "py"]
        ),
        skill!("ruby", "Ruby", Language, parent = "programming"),
        skill!("php", "PHP", Language, parent = "programming"),
        skill!("swift", "Swift", Language, parent = "programming"),
        skill!(
            "objective-c",
            "Objective-C",
            Language,
            parent = "programming",
            aliases = ["objc"]
        ),
        skill!("elixir", "Elixir", Language, parent = "programming"),
        skill!("erlang", "Erlang", Language, parent = "programming"),
        skill!("haskell", "Haskell", Language, parent = "programming"),
        skill!("clojure", "Clojure", Language, parent = "programming"),
        skill!("zig", "Zig", Language, parent = "programming"),
        skill!("sql", "SQL", Language, parent = "programming"),
        skill!(
            "bash",
            "Bash",
            Language,
            parent = "programming",
            aliases = ["shell", "shell scripting", "sh", "zsh"]
        ),
        skill!("r", "R", Language, parent = "programming"),
        skill!("matlab", "MATLAB", Language, parent = "programming"),
        skill!("perl", "Perl", Language, parent = "programming"),
        skill!("lua", "Lua", Language, parent = "programming"),
        skill!("dart", "Dart", Language, parent = "programming"),
        skill!("solidity", "Solidity", Language, parent = "programming"),
        skill!(
            "vhdl",
            "VHDL",
            Language,
            parent = "programming",
            aliases = ["verilog", "systemverilog"]
        ),
        // ---- web frameworks ----------------------------------------------------------
        skill!("frontend", "Frontend Development", Concept),
        skill!(
            "react",
            "React",
            Framework,
            parent = "javascript",
            aliases = ["reactjs", "react.js"]
        ),
        skill!(
            "nextjs",
            "Next.js",
            Framework,
            parent = "react",
            aliases = ["next.js", "next js"]
        ),
        skill!(
            "vue",
            "Vue",
            Framework,
            parent = "javascript",
            aliases = ["vuejs", "vue.js", "vue 3"]
        ),
        skill!(
            "angular",
            "Angular",
            Framework,
            parent = "typescript",
            aliases = ["angularjs", "angular 2+"]
        ),
        skill!(
            "svelte",
            "Svelte",
            Framework,
            parent = "javascript",
            aliases = ["sveltekit"]
        ),
        skill!(
            "nodejs",
            "Node.js",
            Platform,
            parent = "javascript",
            aliases = ["node", "node.js", "nodejs"]
        ),
        skill!(
            "express",
            "Express",
            Framework,
            parent = "nodejs",
            aliases = ["expressjs", "express.js"]
        ),
        skill!("django", "Django", Framework, parent = "python"),
        skill!("flask", "Flask", Framework, parent = "python"),
        skill!("fastapi", "FastAPI", Framework, parent = "python"),
        skill!(
            "rails",
            "Ruby on Rails",
            Framework,
            parent = "ruby",
            aliases = ["ruby on rails", "ror"]
        ),
        skill!(
            "spring",
            "Spring",
            Framework,
            parent = "java",
            aliases = ["spring boot", "springboot"]
        ),
        skill!(
            "dotnet",
            ".NET",
            Framework,
            parent = "csharp",
            aliases = [".net", "net core", ".net core", "asp.net", "aspnet"]
        ),
        skill!("laravel", "Laravel", Framework, parent = "php"),
        skill!("axum", "axum", Framework, parent = "rust"),
        skill!("tokio", "Tokio", Library, parent = "rust"),
        skill!(
            "graphql",
            "GraphQL",
            Concept,
            aliases = ["apollo", "graph ql"]
        ),
        skill!(
            "rest",
            "REST APIs",
            Concept,
            aliases = ["rest api", "restful", "restful apis", "rest apis"]
        ),
        skill!(
            "grpc",
            "gRPC",
            Concept,
            aliases = ["protobuf", "protocol buffers"]
        ),
        skill!(
            "html",
            "HTML",
            Language,
            parent = "frontend",
            aliases = ["html5"]
        ),
        skill!(
            "css",
            "CSS",
            Language,
            parent = "frontend",
            aliases = ["css3", "scss", "sass", "less"]
        ),
        skill!(
            "tailwind",
            "Tailwind CSS",
            Framework,
            parent = "css",
            aliases = ["tailwindcss"]
        ),
        // ---- data --------------------------------------------------------------------
        skill!("databases", "Databases", Concept),
        skill!(
            "postgresql",
            "PostgreSQL",
            Database,
            parent = "databases",
            aliases = ["postgres", "psql", "pgsql"]
        ),
        skill!(
            "mysql",
            "MySQL",
            Database,
            parent = "databases",
            aliases = ["mariadb"]
        ),
        skill!("sqlite", "SQLite", Database, parent = "databases"),
        skill!(
            "mongodb",
            "MongoDB",
            Database,
            parent = "databases",
            aliases = ["mongo"]
        ),
        skill!(
            "redis",
            "Redis",
            Database,
            parent = "databases",
            aliases = ["valkey", "elasticache"]
        ),
        skill!("dynamodb", "DynamoDB", Database, parent = "databases"),
        skill!(
            "cassandra",
            "Cassandra",
            Database,
            parent = "databases",
            aliases = ["scylladb"]
        ),
        skill!(
            "elasticsearch",
            "Elasticsearch",
            Database,
            parent = "databases",
            aliases = ["opensearch", "elastic search", "elk"]
        ),
        skill!("clickhouse", "ClickHouse", Database, parent = "databases"),
        skill!("duckdb", "DuckDB", Database, parent = "databases"),
        skill!("snowflake", "Snowflake", Database, parent = "databases"),
        skill!("bigquery", "BigQuery", Database, parent = "databases"),
        skill!("redshift", "Redshift", Database, parent = "databases"),
        skill!(
            "kafka",
            "Kafka",
            Platform,
            aliases = ["apache kafka", "kinesis", "msk"]
        ),
        skill!("rabbitmq", "RabbitMQ", Platform, aliases = ["amqp"]),
        skill!(
            "spark",
            "Apache Spark",
            Framework,
            aliases = ["pyspark", "apache spark"]
        ),
        skill!(
            "airflow",
            "Airflow",
            Tool,
            aliases = ["apache airflow", "dagster", "prefect"]
        ),
        skill!("dbt", "dbt", Tool),
        skill!(
            "data-engineering",
            "Data Engineering",
            Concept,
            aliases = ["etl", "elt", "data pipelines", "data pipeline"]
        ),
        skill!(
            "data-warehousing",
            "Data Warehousing",
            Concept,
            parent = "data-engineering",
            aliases = ["data warehouse", "dimensional modeling"]
        ),
        // ---- cloud & infrastructure --------------------------------------------------
        skill!("cloud", "Cloud Platforms", Concept),
        skill!(
            "aws",
            "AWS",
            Platform,
            parent = "cloud",
            aliases = ["amazon web services", "ec2", "s3", "lambda"]
        ),
        skill!(
            "gcp",
            "GCP",
            Platform,
            parent = "cloud",
            aliases = ["google cloud", "google cloud platform"]
        ),
        skill!(
            "azure",
            "Azure",
            Platform,
            parent = "cloud",
            aliases = ["microsoft azure"]
        ),
        skill!(
            "kubernetes",
            "Kubernetes",
            Tool,
            aliases = ["k8s", "eks", "gke", "aks", "openshift"]
        ),
        skill!(
            "docker",
            "Docker",
            Tool,
            aliases = ["containers", "containerization", "podman", "oci"]
        ),
        skill!(
            "terraform",
            "Terraform",
            Tool,
            aliases = ["opentofu", "hcl"]
        ),
        skill!(
            "ansible",
            "Ansible",
            Tool,
            aliases = ["chef", "puppet", "saltstack"]
        ),
        skill!("helm", "Helm", Tool, parent = "kubernetes"),
        skill!(
            "linux",
            "Linux",
            Platform,
            aliases = ["unix", "ubuntu", "debian", "rhel", "centos"]
        ),
        skill!(
            "devops",
            "DevOps",
            Methodology,
            aliases = ["sre", "site reliability", "platform engineering"]
        ),
        skill!(
            "cicd",
            "CI/CD",
            Methodology,
            parent = "devops",
            aliases = [
                "ci/cd",
                "ci cd",
                "continuous integration",
                "continuous delivery",
                "continuous deployment"
            ]
        ),
        skill!("jenkins", "Jenkins", Tool, parent = "cicd"),
        skill!(
            "github-actions",
            "GitHub Actions",
            Tool,
            parent = "cicd",
            aliases = ["gitlab ci", "circleci", "travis ci", "buildkite"]
        ),
        skill!(
            "observability",
            "Observability",
            Concept,
            parent = "devops",
            aliases = ["monitoring", "alerting", "telemetry"]
        ),
        skill!(
            "prometheus",
            "Prometheus",
            Tool,
            parent = "observability",
            aliases = ["grafana", "thanos"]
        ),
        skill!(
            "datadog",
            "Datadog",
            Tool,
            parent = "observability",
            aliases = ["new relic", "splunk", "sumo logic"]
        ),
        skill!(
            "opentelemetry",
            "OpenTelemetry",
            Tool,
            parent = "observability",
            aliases = ["otel", "open telemetry"]
        ),
        skill!(
            "nginx",
            "nginx",
            Tool,
            aliases = ["apache httpd", "haproxy", "envoy", "traefik"]
        ),
        // ---- architecture & practice -------------------------------------------------
        skill!(
            "distributed-systems",
            "Distributed Systems",
            Concept,
            aliases = ["distributed system", "distributed computing"]
        ),
        skill!(
            "microservices",
            "Microservices",
            Concept,
            parent = "distributed-systems",
            aliases = ["micro-services", "service oriented architecture", "soa"]
        ),
        skill!(
            "system-design",
            "System Design",
            Concept,
            aliases = [
                "software architecture",
                "systems design",
                "architecture design"
            ]
        ),
        skill!(
            "scalability",
            "Scalability",
            Concept,
            parent = "distributed-systems",
            aliases = ["high availability", "high throughput", "load balancing"]
        ),
        skill!(
            "performance",
            "Performance Engineering",
            Concept,
            aliases = [
                "performance optimization",
                "performance tuning",
                "profiling",
                "low latency"
            ]
        ),
        skill!(
            "concurrency",
            "Concurrency",
            Concept,
            aliases = [
                "multithreading",
                "parallelism",
                "async programming",
                "asynchronous programming"
            ]
        ),
        skill!(
            "testing",
            "Testing",
            Methodology,
            aliases = [
                "unit testing",
                "integration testing",
                "test automation",
                "tdd",
                "automated testing"
            ]
        ),
        skill!("code-review", "Code Review", Methodology),
        skill!(
            "agile",
            "Agile",
            Methodology,
            aliases = ["scrum", "kanban", "sprint planning"]
        ),
        skill!(
            "git",
            "Git",
            Tool,
            aliases = ["github", "gitlab", "version control", "bitbucket"]
        ),
        skill!(
            "jira",
            "Jira",
            Tool,
            aliases = ["confluence", "linear", "asana"]
        ),
        skill!(
            "security",
            "Security",
            Concept,
            aliases = ["application security", "appsec", "infosec", "cybersecurity"]
        ),
        skill!(
            "cryptography",
            "Cryptography",
            Concept,
            parent = "security",
            aliases = ["encryption", "pki", "tls"]
        ),
        skill!(
            "authentication",
            "Authentication",
            Concept,
            parent = "security",
            aliases = ["oauth", "oauth2", "oidc", "saml", "sso", "authorization"]
        ),
        skill!(
            "compliance",
            "Compliance",
            Domain,
            aliases = [
                "soc 2",
                "soc2",
                "hipaa",
                "pci dss",
                "gdpr",
                "fedramp",
                "iso 27001"
            ]
        ),
        // ---- ml / ai -----------------------------------------------------------------
        skill!(
            "machine-learning",
            "Machine Learning",
            Concept,
            aliases = ["ml", "deep learning", "neural networks"]
        ),
        skill!(
            "pytorch",
            "PyTorch",
            Framework,
            parent = "machine-learning",
            aliases = ["torch"]
        ),
        skill!(
            "tensorflow",
            "TensorFlow",
            Framework,
            parent = "machine-learning",
            aliases = ["keras"]
        ),
        skill!(
            "llm",
            "Large Language Models",
            Concept,
            parent = "machine-learning",
            aliases = [
                "llms",
                "large language model",
                "generative ai",
                "genai",
                "gpt",
                "rag",
                "prompt engineering"
            ]
        ),
        skill!(
            "mlops",
            "MLOps",
            Methodology,
            parent = "machine-learning",
            aliases = ["ml ops", "model deployment", "feature store"]
        ),
        skill!(
            "nlp",
            "NLP",
            Concept,
            parent = "machine-learning",
            aliases = ["natural language processing"]
        ),
        skill!(
            "computer-vision",
            "Computer Vision",
            Concept,
            parent = "machine-learning",
            aliases = ["cv", "image processing", "opencv"]
        ),
        skill!(
            "data-science",
            "Data Science",
            Concept,
            aliases = [
                "statistics",
                "statistical analysis",
                "a/b testing",
                "experimentation"
            ]
        ),
        skill!(
            "pandas",
            "pandas",
            Library,
            parent = "python",
            aliases = ["numpy", "scipy", "scikit-learn", "sklearn"]
        ),
        skill!(
            "lightgbm",
            "LightGBM",
            Library,
            parent = "machine-learning",
            aliases = ["light gbm", "xgboost", "catboost", "gradient boosting"]
        ),
        skill!(
            "dataiku",
            "Dataiku",
            Platform,
            aliases = ["dataiku dss", "dss"]
        ),
        skill!(
            "tableau",
            "Tableau",
            Tool,
            aliases = ["tableau server", "tableau desktop"]
        ),
        skill!(
            "databricks",
            "Databricks",
            Platform,
            aliases = ["databricks sql"]
        ),
        // ---- mobile & embedded -------------------------------------------------------
        skill!("mobile", "Mobile Development", Concept),
        skill!(
            "ios",
            "iOS",
            Platform,
            parent = "mobile",
            aliases = ["swiftui", "uikit"]
        ),
        skill!(
            "android",
            "Android",
            Platform,
            parent = "mobile",
            aliases = ["jetpack compose"]
        ),
        skill!(
            "react-native",
            "React Native",
            Framework,
            parent = "react",
            aliases = ["reactnative"]
        ),
        skill!("flutter", "Flutter", Framework, parent = "dart"),
        skill!(
            "embedded",
            "Embedded Systems",
            Concept,
            aliases = ["firmware", "rtos", "bare metal", "microcontroller"]
        ),
        skill!(
            "robotics",
            "Robotics",
            Domain,
            aliases = ["ros", "ros2", "motion planning", "slam"]
        ),
        skill!(
            "realtime",
            "Real-time Systems",
            Concept,
            parent = "embedded",
            aliases = ["real-time", "real time systems", "hard real-time"]
        ),
        // ---- leadership & soft -------------------------------------------------------
        skill!(
            "mentoring",
            "Mentoring",
            Soft,
            aliases = ["mentorship", "coaching", "developing engineers"]
        ),
        skill!(
            "technical-leadership",
            "Technical Leadership",
            Soft,
            aliases = ["tech lead", "leading projects", "technical direction"]
        ),
        skill!(
            "people-management",
            "People Management",
            Soft,
            aliases = [
                "managing engineers",
                "performance management",
                "hiring",
                "team management"
            ]
        ),
        skill!(
            "communication",
            "Communication",
            Soft,
            aliases = [
                "written communication",
                "verbal communication",
                "presentation skills",
                "stakeholder management"
            ]
        ),
        skill!(
            "collaboration",
            "Collaboration",
            Soft,
            aliases = ["teamwork", "cross-functional collaboration", "team player"]
        ),
        skill!(
            "problem-solving",
            "Problem Solving",
            Soft,
            aliases = [
                "analytical skills",
                "critical thinking",
                "troubleshooting",
                "debugging"
            ]
        ),
        skill!(
            "ownership",
            "Ownership",
            Soft,
            aliases = [
                "self-starter",
                "autonomy",
                "works independently",
                "bias for action"
            ]
        ),
        skill!(
            "product-sense",
            "Product Sense",
            Soft,
            aliases = ["product thinking", "customer focus", "user empathy"]
        ),
        // ---- domains -----------------------------------------------------------------
        skill!(
            "fintech",
            "Fintech",
            Domain,
            aliases = [
                "financial services",
                "payments",
                "banking",
                "trading systems"
            ]
        ),
        skill!(
            "healthcare",
            "Healthcare",
            Domain,
            aliases = ["health tech", "healthtech", "medical devices", "ehr"]
        ),
        skill!(
            "ecommerce",
            "E-commerce",
            Domain,
            aliases = ["e-commerce", "retail", "marketplace"]
        ),
        skill!(
            "gaming",
            "Gaming",
            Domain,
            aliases = ["game development", "unity", "unreal engine"]
        ),
        skill!(
            "defense",
            "Defense",
            Domain,
            aliases = ["defence", "govtech", "aerospace", "dod"]
        ),
        // ---- certifications ----------------------------------------------------------
        skill!(
            "aws-certification",
            "AWS Certification",
            Certification,
            aliases = [
                "aws certified solutions architect",
                "aws certified developer"
            ]
        ),
        skill!(
            "cka",
            "Certified Kubernetes Administrator",
            Certification,
            aliases = ["cka", "ckad"]
        ),
        skill!("cissp", "CISSP", Certification),
        skill!("pmp", "PMP", Certification),
        skill!(
            "scrum-master",
            "Certified Scrum Master",
            Certification,
            aliases = ["csm", "psm"]
        ),
        // ---- human languages ---------------------------------------------------------
        skill!("english", "English", LanguageHuman),
        skill!("spanish", "Spanish", LanguageHuman),
        skill!("french", "French", LanguageHuman),
        skill!("german", "German", LanguageHuman),
        skill!("mandarin", "Mandarin", LanguageHuman, aliases = ["chinese"]),
        skill!("japanese", "Japanese", LanguageHuman),
        skill!("portuguese", "Portuguese", LanguageHuman),
    ]
});

/// Alias → slug, built once. Includes each skill's own name and slug so the canonical
/// spelling always resolves.
static ALIAS_INDEX: Lazy<HashMap<String, &'static str>> = Lazy::new(|| {
    let mut map = HashMap::new();
    for skill in SEED_SKILLS.iter() {
        map.insert(alias_key(skill.name), skill.slug);
        map.insert(alias_key(skill.slug), skill.slug);
        for alias in skill.aliases {
            // Aliases are declared in order of preference; an earlier skill keeps a
            // contested alias so the taxonomy is deterministic regardless of hash order.
            map.entry(alias_key(alias)).or_insert(skill.slug);
        }
    }
    map
});

static BY_SLUG: Lazy<HashMap<&'static str, &'static SeedSkill>> =
    Lazy::new(|| SEED_SKILLS.iter().map(|s| (s.slug, s)).collect());

/// Normalize an alias for lookup: lowercase, punctuation-insensitive, whitespace-collapsed.
/// `"Node.js"`, `"node js"` and `"NODEJS"` all produce the same key.
fn alias_key(input: &str) -> String {
    let lowered = input.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_space = true;
    for ch in lowered.chars() {
        // `+` and `#` are load-bearing in language names and must survive.
        if ch.is_alphanumeric() || ch == '+' || ch == '#' {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

/// Resolve a phrase to a taxonomy slug.
///
/// Matches the longest recognized alias appearing in the phrase, so
/// `"3+ years of hands-on Kubernetes in production"` resolves to `kubernetes`.
pub fn resolve(phrase: &str) -> Option<&'static str> {
    let key = alias_key(phrase);
    if let Some(slug) = ALIAS_INDEX.get(&key) {
        return Some(slug);
    }

    // Sliding window over the phrase, longest first. Longest-match matters: without it
    // "React Native" resolves to "react", and "Node.js" to nothing.
    let words: Vec<&str> = key.split(' ').filter(|w| !w.is_empty()).collect();
    let max_span = words.len().min(4);
    for span in (1..=max_span).rev() {
        for start in 0..=words.len().saturating_sub(span) {
            let candidate = words[start..start + span].join(" ");
            if let Some(slug) = ALIAS_INDEX.get(&candidate) {
                return Some(slug);
            }
        }
    }
    None
}

/// Every slug on the path from `slug` up to its root, `slug` first.
///
/// This is what lets React evidence count partially toward a JavaScript requirement.
pub fn ancestors(slug: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut current = BY_SLUG.get(slug).copied();
    let mut guard = 0;
    while let Some(skill) = current {
        out.push(skill.slug);
        // The seed data is hand-written; a cycle would otherwise hang the scorer.
        guard += 1;
        if guard > 16 {
            break;
        }
        current = skill.parent.and_then(|p| BY_SLUG.get(p).copied());
    }
    out
}

/// How much credit `have` earns against `want`, following the hierarchy upward only.
///
/// `1.0` for an exact match, decayed per hop for a descendant, and `0.0` when `have` is an
/// *ancestor* of `want`: knowing JavaScript is not knowing React, and pretending otherwise
/// would inflate every score.
pub fn hierarchy_credit(have: &str, want: &str) -> f32 {
    use jobseeker_core::domain::requirement::HIERARCHY_DECAY_PER_HOP;
    if have == want {
        return 1.0;
    }
    ancestors(have)
        .iter()
        .position(|s| *s == want)
        .map(|hops| HIERARCHY_DECAY_PER_HOP.powi(hops as i32))
        .unwrap_or(0.0)
}

pub fn get(slug: &str) -> Option<&'static SeedSkill> {
    BY_SLUG.get(slug).copied()
}

/// Extract every skill mentioned in a block of text, deduplicated and in order of first
/// appearance. Used to tag accomplishments and to enrich requirements.
pub fn extract_all(text: &str) -> Vec<&'static str> {
    let key = alias_key(text);
    let words: Vec<&str> = key.split(' ').filter(|w| !w.is_empty()).collect();
    let mut out: Vec<&'static str> = Vec::new();
    let mut i = 0usize;
    while i < words.len() {
        let mut matched = None;
        for span in (1..=4.min(words.len() - i)).rev() {
            let candidate = words[i..i + span].join(" ");
            if let Some(slug) = ALIAS_INDEX.get(&candidate) {
                matched = Some((*slug, span));
                break;
            }
        }
        match matched {
            Some((slug, span)) => {
                if !out.contains(&slug) {
                    out.push(slug);
                }
                i += span;
            }
            None => i += 1,
        }
    }
    out
}

/// A phrase that looks like a skill but is not in the taxonomy, normalized for the
/// `skill_candidate` review queue. Unknown skills are queued, not dropped.
pub fn candidate_key(phrase: &str) -> String {
    slugify(&alias_key(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_and_slugs_always_resolve() {
        for skill in SEED_SKILLS.iter() {
            assert_eq!(
                resolve(skill.name),
                Some(skill.slug),
                "name: {}",
                skill.name
            );
            assert_eq!(
                resolve(skill.slug),
                Some(skill.slug),
                "slug: {}",
                skill.slug
            );
        }
    }

    #[test]
    fn every_declared_parent_exists() {
        for skill in SEED_SKILLS.iter() {
            if let Some(parent) = skill.parent {
                assert!(
                    get(parent).is_some(),
                    "{} declares a missing parent {parent}",
                    skill.slug
                );
            }
        }
    }

    #[test]
    fn the_hierarchy_is_acyclic() {
        for skill in SEED_SKILLS.iter() {
            let chain = ancestors(skill.slug);
            let unique: std::collections::HashSet<_> = chain.iter().collect();
            assert_eq!(chain.len(), unique.len(), "cycle through {}", skill.slug);
        }
    }

    #[test]
    fn aliases_fold_onto_one_node() {
        assert_eq!(resolve("K8s"), Some("kubernetes"));
        assert_eq!(resolve("k8s"), Some("kubernetes"));
        assert_eq!(resolve("EKS"), Some("kubernetes"));
        assert_eq!(resolve("Golang"), Some("go"));
        assert_eq!(resolve("Postgres"), Some("postgresql"));
        assert_eq!(resolve("TS"), Some("typescript"));
    }

    #[test]
    fn punctuation_and_case_do_not_matter() {
        for spelling in ["Node.js", "node js", "NODEJS", "Node.JS"] {
            assert_eq!(resolve(spelling), Some("nodejs"), "spelling: {spelling}");
        }
        assert_eq!(resolve("C++"), Some("cpp"));
        assert_eq!(resolve("c#"), Some("csharp"));
        assert_eq!(resolve(".NET Core"), Some("dotnet"));
    }

    #[test]
    fn a_skill_is_found_inside_a_requirement_sentence() {
        assert_eq!(
            resolve("3+ years of hands-on Kubernetes in production"),
            Some("kubernetes")
        );
        assert_eq!(
            resolve("Strong experience with PostgreSQL"),
            Some("postgresql")
        );
    }

    #[test]
    fn the_longest_alias_wins_so_specific_skills_are_not_swallowed() {
        assert_eq!(
            resolve("React Native"),
            Some("react-native"),
            "must not collapse to plain React"
        );
        assert_eq!(resolve("Ruby on Rails"), Some("rails"));
        assert_eq!(resolve("Google Cloud Platform"), Some("gcp"));
    }

    #[test]
    fn evidence_flows_up_the_hierarchy_but_never_down() {
        // React experience is partial evidence of JavaScript.
        let up = hierarchy_credit("react", "javascript");
        assert!(up > 0.0 && up < 1.0, "got {up}");

        // JavaScript experience is not evidence of React. This asymmetry is the whole
        // point: the reverse would let a generalist claim every framework.
        assert_eq!(hierarchy_credit("javascript", "react"), 0.0);
    }

    #[test]
    fn credit_decays_with_distance() {
        let one_hop = hierarchy_credit("typescript", "javascript");
        let two_hops = hierarchy_credit("typescript", "programming");
        assert!(one_hop > two_hops, "{one_hop} should exceed {two_hops}");
        assert_eq!(hierarchy_credit("rust", "rust"), 1.0);
        assert_eq!(
            hierarchy_credit("rust", "python"),
            0.0,
            "siblings share nothing"
        );
    }

    #[test]
    fn ancestors_run_from_the_skill_to_its_root() {
        assert_eq!(
            ancestors("nextjs"),
            vec!["nextjs", "react", "javascript", "programming"]
        );
        assert_eq!(ancestors("rust"), vec!["rust", "programming"]);
        assert!(ancestors("not-a-skill").is_empty());
    }

    #[test]
    fn all_skills_in_a_sentence_are_extracted_once_each() {
        let found = extract_all(
            "Built a Rust service on Kubernetes with PostgreSQL and Kafka, deployed via Terraform",
        );
        for expected in ["rust", "kubernetes", "postgresql", "kafka", "terraform"] {
            assert!(found.contains(&expected), "missing {expected} in {found:?}");
        }
        let repeated = extract_all("Rust, more Rust, and even more Rust");
        assert_eq!(repeated, vec!["rust"], "each skill appears once");
    }

    #[test]
    fn unknown_phrases_resolve_to_nothing_and_become_candidates() {
        assert_eq!(resolve("Blorptron 9000"), None);
        assert_eq!(candidate_key("Blorptron 9000!"), "blorptron-9000");
    }

    #[test]
    fn slugs_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for skill in SEED_SKILLS.iter() {
            assert!(seen.insert(skill.slug), "duplicate slug {}", skill.slug);
        }
    }
}
