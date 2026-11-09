use std::{env, fs};

fn main() {
    println!("cargo:rustc-link-lib=pam");
    libpam_sys_impls::enable_pam_impl_cfg();

    let pam_impl = libpam_sys_impls::build_target_impl();
    let impl_str = pam_impl
        .map(|i| format!("{i:?}"))
        .unwrap_or("[undefined]".into());
    println!("cargo:rustc-env=LIBPAMSYS_IMPL={impl_str}");
    let output = match pam_impl {
        None => "".into(),
        Some(pam_impl) => {
            format!(
                "\
                /// The implementation of PAM this library was built against.
                pub const CURRENT: PamImpl = PamImpl::{pam_impl:?};
                /// The name of the PAM implementation this library was built
                /// against, as a string.
                #[macro_export]
                macro_rules! pam_impl_name {{ () => {{ \"{pam_impl:?}\" }} }}
                pub(crate) use pam_impl_name;
                "
            )
        }
    };
    let outfile = format!(
        "{out}/pam_impl_consts.rs",
        out = env::var("OUT_DIR").expect("missing OUT_DIR env var")
    );
    fs::write(outfile, output).expect("couldn't write output file");
}
