use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Build {
    out_dir: Option<PathBuf>,
    target: Option<String>,
    host: Option<String>,
    // Max number of Lua stack slots
    max_stack_size: Option<usize>,
    // Use longjmp instead of C++ exceptions
    use_longjmp: Option<bool>,
}

pub struct Artifacts {
    lib_dir: PathBuf,
    libs: Vec<String>,
    cpp_stdlib: Option<String>,
}

impl Build {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Build {
        Build {
            out_dir: env::var_os("OUT_DIR").map(|s| PathBuf::from(s).join("pluto-build")),
            target: env::var("TARGET").ok(),
            host: env::var("HOST").ok(),
            max_stack_size: None,
            use_longjmp: None,
        }
    }

    pub fn out_dir<P: AsRef<Path>>(&mut self, path: P) -> &mut Build {
        self.out_dir = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn target(&mut self, target: &str) -> &mut Build {
        self.target = Some(target.to_string());
        self
    }

    pub fn host(&mut self, host: &str) -> &mut Build {
        self.host = Some(host.to_string());
        self
    }

    pub fn set_max_stack_size(&mut self, size: usize) -> &mut Build {
        self.max_stack_size = Some(size);
        self
    }

    pub fn use_longjmp(&mut self, r#use: bool) -> &mut Build {
        self.use_longjmp = Some(r#use);
        self
    }

    pub fn build(&mut self) -> Artifacts {
        let target = &self.target.as_ref().expect("TARGET not set")[..];
        let host = &self.host.as_ref().expect("HOST not set")[..];
        let out_dir = self.out_dir.as_ref().expect("OUT_DIR not set");

        let pluto_source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("pluto");
        let soup_source_dir = pluto_source_dir.join("vendor").join("Soup");

        // Cleanup
        if out_dir.exists() {
            fs::remove_dir_all(out_dir).unwrap();
        }

        // Configure C++
        let mut config = cc::Build::new();
        config
            .target(target)
            .host(host)
            .warnings(false)
            .cargo_metadata(false)
            .std("c++17")
            .flag_if_supported("-fvisibility=hidden")
            .flag_if_supported("-fno-rtti")
            .flag_if_supported("-Wno-multichar")
            .cpp(true);

        if let Some(max_stack_size) = self.max_stack_size {
            config.define("LUAI_MAXSTACK", &*max_stack_size.to_string());
        }

        if let Some(true) = self.use_longjmp {
            config.define("LUA_USE_LONGJMP", "1");
        }

        if cfg!(debug_assertions) {
            config.define("LUA_USE_APICHECK", None);
        } else {
            config.define("NDEBUG", None);
            config.opt_level(2);
            // this flag allows compiler to lower sqrt() into a single CPU instruction
            config.flag_if_supported("-fno-math-errno");
        }

        // Build Soup
        let soup_lib_name = "soup";
        let mut soup_config = config.clone();
        soup_config.add_files_by_ext(&soup_source_dir.join("soup"), "cpp");
        match target {
            _ if target.contains("x86_64") => {
                soup_config
                    .define("SOUP_USE_INTRIN", None)
                    .add_files_by_ext(&soup_source_dir.join("Intrin"), "cpp")
                    .flag_if_supported("-maes")
                    .flag_if_supported("-mpclmul")
                    .flag_if_supported("-mrdrnd")
                    .flag_if_supported("-mrdseed")
                    .flag_if_supported("-msha")
                    .flag_if_supported("-msse4.1");
            }
            _ if target.contains("aarch64") => {
                soup_config
                    .define("SOUP_USE_INTRIN", None)
                    .add_files_by_ext(&soup_source_dir.join("Intrin"), "cpp")
                    .flag_if_supported("-march=armv8-a+crypto+crc");
            }
            _ => {}
        }
        soup_config.out_dir(out_dir).compile(soup_lib_name);

        // Build Pluto
        let pluto_lib_name = "pluto";
        config
            .add_files_by_ext(&pluto_source_dir, "cpp")
            .out_dir(out_dir)
            .compile(pluto_lib_name);

        Artifacts {
            lib_dir: out_dir.to_path_buf(),
            libs: vec![pluto_lib_name.to_string(), soup_lib_name.to_string()],
            cpp_stdlib: Self::get_cpp_link_stdlib(target, host),
        }
    }

    /// Returns the C++ standard library:
    /// 1) Uses `CXXSTDLIB` environment variable if set
    /// 2) The default `c++` for OS X and BSDs
    /// 3) `c++_shared` for Android
    /// 4) `None` for MSVC
    /// 5) `stdc++` for anything else.
    ///
    /// Inspired by the `cc` crate.
    fn get_cpp_link_stdlib(target: &str, host: &str) -> Option<String> {
        // Try to get value from the `CXXSTDLIB` env variable
        let kind = if host == target { "HOST" } else { "TARGET" };
        let res = env::var(format!("CXXSTDLIB_{target}"))
            .or_else(|_| env::var(format!("CXXSTDLIB_{}", target.replace('-', "_"))))
            .or_else(|_| env::var(format!("{kind}_CXXSTDLIB")))
            .or_else(|_| env::var("CXXSTDLIB"))
            .ok();
        if res.is_some() {
            return res;
        }

        if target.contains("msvc") {
            None
        } else if target.contains("apple") | target.contains("freebsd") | target.contains("openbsd")
        {
            Some("c++".to_string())
        } else if target.contains("android") {
            Some("c++_shared".to_string())
        } else {
            Some("stdc++".to_string())
        }
    }
}

impl Artifacts {
    pub fn lib_dir(&self) -> &Path {
        &self.lib_dir
    }

    pub fn libs(&self) -> &[String] {
        &self.libs
    }

    pub fn print_cargo_metadata(&self) {
        println!("cargo:rustc-link-search=native={}", self.lib_dir.display());
        for lib in self.libs.iter() {
            println!("cargo:rustc-link-lib=static={}", lib);
        }
        if let Some(ref cpp_stdlib) = self.cpp_stdlib {
            println!("cargo:rustc-link-lib={}", cpp_stdlib);
        }
    }
}

trait AddFilesByExt {
    fn add_files_by_ext(&mut self, dir: &Path, ext: &str) -> &mut Self;
}

impl AddFilesByExt for cc::Build {
    fn add_files_by_ext(&mut self, dir: &Path, ext: &str) -> &mut Self {
        for entry in fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension() == Some(ext.as_ref()))
        {
            self.file(entry.path());
        }
        self
    }
}
