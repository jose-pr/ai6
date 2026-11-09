//! A place to stick documentation stuff.

/// Generates text lists of reference links for docs.
///
/// Use this with the other doc macros for the correct link names.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::{linklist, stdlinks};
/// /// Here is a list of links:
/// ///
/// #[doc = linklist!(pam_get_authtok: man7, manbsd)]
/// ///
/// /// The links are defined in the `stdlinks!` invocation below:
/// ///
/// #[doc = stdlinks!(3 pam_get_authtok)]
/// # fn do_whatever() {}
/// ```
macro_rules! linklist {
    ($func:ident: adg$(, $rest:ident)*) => {
        concat!(
            "- [Application Developers' Guide on `", stringify!($func), "`][adg]\n",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident: mwg$(, $rest:ident)*) => {
        concat!(
            "- [Module Writers' Guide on `", stringify!($func), "`][mwg]\n",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident: _std$(, $rest:ident)*) => {
        $crate::_doc::linklist!($func: man7, manbsd, mansun, xsso$(, $rest)*)
    };
    ($func:ident: man7$(, $rest:ident)*) => {
        concat!(
            "- [Linux-PAM manpage for `", stringify!($func), "`][man7]\n",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident: manbsd$(, $rest:ident)*) => {
        concat!(
            "- [OpenPAM manpage for `", stringify!($func), "`][manbsd]\n",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident: mansun$(, $rest:ident)*) => {
        concat!(
            "- [Illumos manpage for `", stringify!($func), "`][mansun]\n",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident: xsso$(, $rest:ident)*) => {
        concat!(
            "- [X/SSO spec for `", stringify!($func), "`][xsso]",
            $crate::_doc::linklist!($func: $($rest),*)
        )
    };
    ($func:ident:$(,)?) => { "" };
}

/// Generates a Markdown link reference to one of the PAM guides.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::{guide};
/// /// See [the guide][mwg].
/// ///
/// #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_get_user")]
/// # fn do_whatever() {}
/// ```
macro_rules! guide {
    ($name:ident: $page_link:literal) => {
        concat!(
            "[",
            stringify!($name),
            "]: https://www.chiark.greenend.org.uk/doc/libpam-doc/html/",
            $page_link
        )
    };
}

/// Generates a Markdown link reference to the Linux man pages on man7.org.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::man7;
/// /// This contains a [link to the man page for malloc][man7].
/// #[doc = man7!(3 malloc)]
/// # fn do_whatever() {}
///
/// /// This contains both a link to the ["structure" section of `hgrc`][man7]
/// /// and a link to the ["environment" section of `systemd`][systemd_env].
/// ///
/// #[doc = man7!(5 hgrc "STRUCTURE")]
/// #[doc = man7!(systemd_env: 1 systemd "ENVIRONMENT")]
/// # fn do_whatever2() {}
/// ```
macro_rules! man7 {
    ($n:literal $fn:ident $($anchor:literal)?) => {
        $crate::_doc::man7!(man7: $n $fn $($anchor)?)
    };
    ($name:ident: $n:literal $fn:ident $($anchor:literal)?) => {
        concat!(
            "[", stringify!($name), "]: ",
            "https://man7.org/linux/man-pages/man", $n, "/",
            stringify!($fn), ".", $n, ".html", $("#", $anchor)?
        )
    };
}

/// Generates a Markdown link reference to the NetBSD man pages.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::manbsd;
/// // Both of these formulations create a link named `manbsd`.
/// #[doc = manbsd!(3 fn_name)]
/// #[doc = manbsd!(5 thing_name "SECTION")]
/// // This one creates a link named `link_name`.
/// #[doc = manbsd!(link_name: 1 prog_name "SECTION")]
/// # fn do_whatever() {}
/// ```
macro_rules! manbsd {
    ($n:literal $func:ident $($anchor:literal)?) => {
        $crate::_doc::manbsd!(manbsd: $n $func $($anchor)?)
    };
    ($name:ident: $n:literal $func:ident $($anchor:literal)?) => {
        concat!("[", stringify!($name), "]: ",
            "https://man.netbsd.org/", stringify!($func), ".", $n,
            $("#", $anchor)?
        )
    };
}

/// Generates a Markdown link reference to the SmartOS man pages.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::mansun;
/// // Both of these formulations create a link named `mansun`.
/// #[doc = mansun!(3pam fn_name)]
/// #[doc = mansun!(5 "a.out" "synopsis")]
/// // This one creates a link named `link_name`.
/// #[doc = mansun!(link_name: 1 alias "examples")]
/// # fn do_whatever() {}
/// ```
macro_rules! mansun {
    ($n:literal $func:ident $($anchor:literal)?) => {
        $crate::_doc::mansun!(mansun: [$n ""] $func $($anchor)?)
    };
    ($n:literal $func:literal $($anchor:literal)?) => {
        $crate::_doc::mansun!(mansun: [$n ""] $func $($anchor)?)
    };
    ([$n:literal $sect:literal] $func:ident $($anchor:literal)?) => {
        $crate::_doc::mansun!(mansun: [$n $sect] $func $($anchor)?)
    };
    ($name:ident: $n:literal $func:ident $($anchor:literal)?) => {
        $crate::_doc::mansun!($name: [$n ""] $func $($anchor)?)
    };
    ($name:ident: $n:literal $func:literal $($anchor:literal)?) => {
        $crate::_doc::mansun!($name: [$n ""] ($func) $($anchor)?)
    };
    ($name:ident: [$n:literal $sect:literal] $func:ident $($anchor:literal)?) => {
        $crate::_doc::mansun!($name: [$n $sect] (stringify!($func)) $($anchor)?)
    };
    ($name:ident: [$n:literal $sect:literal] ($func:expr) $($anchor:literal)?) => {
        concat!("[", stringify!($name), "]: ",
            "https://smartos.org/man/", $n, $sect, "/", $func,
            $("#", $anchor)?
        )
    };
}

/// Generates a Markdown link reference to the X/SSO specification.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::xsso;
/// /// This docstring will [link to the X/SSO spec for `pam_set_item`][xsso].
/// ///
/// #[doc = xsso!(pam_set_item)]
/// # fn link_one() {}
///
/// /// This docstring will link to [`some_page`][xsso].
/// /// I can also link to [the table of contents][spec_toc].
/// ///
/// #[doc = xsso!("some_page.htm#section-id")]
/// #[doc = xsso!(spec_toc: "toc.htm")]
/// # fn do_whatever() {}
/// ```
macro_rules! xsso {
    ($func:ident) => { $crate::_doc::xsso!(xsso: concat!(stringify!($func), ".htm")) };
    ($page:literal) => { $crate::_doc::xsso!(xsso: $page) };
    ($name:ident: $page:expr) => {
        concat!("[", stringify!($name), "]: https://pubs.opengroup.org/onlinepubs/8329799/", $page)
    };
}

/// Generates Markdown link references to Linux-PAM, OpenPAM, and X/SSO.
///
/// A shortcut to `man7!`, `manbsd!`, and `xsso!`.
///
/// # Examples
///
/// ```ignore
/// # use nonstick::_doc::stdlinks;
/// /// Check out [this][man7], [that][manbsd], or [the other][xsso].
/// ///
/// #[doc = stdlinks!(3 pam_get_item)]
/// # fn do_whatever() {}
/// ```
macro_rules! stdlinks {
    ($n:literal $func:ident) => {
        concat!(
            $crate::_doc::man7!($n $func), "\n",
            $crate::_doc::manbsd!($n $func), "\n",
            $crate::_doc::mansun!([$n "pam"] $func), "\n",
            $crate::_doc::xsso!($func))
    };
}

pub(crate) use {guide, linklist, man7, manbsd, mansun, stdlinks, xsso};
