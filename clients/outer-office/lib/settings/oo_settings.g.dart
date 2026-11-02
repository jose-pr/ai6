// Generated from `OoSettings` in crates/ai6/src/bootstrap/oo.rs; a test there fails when it
// differs. To write it: cargo run -p ai6 --features cli --example settings_reference

/// `[oo]`'s defaults, by table and key.
abstract final class OoDefaults {
  static const chatMinWidth = 320.0;
  static const chatMaxWidth = 720.0;
  static const chatWidth = 400.0;
  static const chatWidthStep = 16.0;
  static const chatMinPageWidth = 360.0;
  static const chatTextWidth = 960.0;
  static const chatWindowWidth = 520.0;
  static const chatWindowHeight = 760.0;
  static const chatTransitionMs = 200;
  static const chatConnectTimeout = Duration(seconds: 10);
  static const chatRetryDelay = Duration(seconds: 1);
  static const chatRetryMaxDelay = Duration(seconds: 16);
  static const routesTextBytes = 128;
  static const artMaxWidth = 3840;
  static const refreshSessions = Duration(seconds: 3);
  static const refreshOverview = Duration(seconds: 10);
  static const refreshTimeline = Duration(seconds: 10);
  static const refreshPools = Duration(seconds: 15);
  static const refreshSlow = Duration(seconds: 15);
  static const refreshProfile = Duration(seconds: 60);
  static const refreshHidden = Duration(seconds: 60);
  static const refreshSilentIntervals = 3;
  static const refreshRenewBefore = Duration(seconds: 600);
  static const refreshRenewRetry = Duration(seconds: 30);
  static const controlsWait = Duration(seconds: 10);
  static const controlsPoll = Duration(milliseconds: 500);
  static const controlsConfirming = Duration(seconds: 30);
  static const pagesTimeline = 50;
  static const pagesAudit = 100;
  static const pagesSummary = 5;
  static const pagesSearch = 24;
  static const terminalScrollbackLines = 10000;
  static const terminalConnectTimeout = Duration(seconds: 10);
  static const terminalRetryDelay = Duration(seconds: 1);
  static const terminalRetryMaxDelay = Duration(seconds: 16);
  static const limitsMissionTitleChars = 512;
  static const limitsMissionPromptChars = 65536;
  static const limitsReasonChars = 512;
  static const assetsCdn = 'none';
}

/// `[oo]`'s schema and form, as `ai6 config schema oo` states them.
const ooSettingsJson = r'''
{
  "schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
      "appearance": {
        "type": "object",
        "properties": {
          "theme": {
            "type": [
              "string",
              "null"
            ],
            "title": "Theme",
            "description": "The theme a person who has chosen none starts with, or `system`; what a person chooses wins. Default: `night-ops`.",
            "examples": [
              "night-ops",
              "black-ops",
              "dusk",
              "classic",
              "light",
              "red-file",
              "system"
            ]
          }
        },
        "additionalProperties": false,
        "title": "Appearance",
        "description": "`[oo.appearance]`.",
        "default": {}
      },
      "chat": {
        "type": "object",
        "properties": {
          "min_width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Narrowest docked chat",
            "description": "How narrow the docked chat may be dragged.",
            "default": 320
          },
          "max_width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Widest docked chat",
            "description": "How wide the docked chat may be dragged.",
            "default": 720
          },
          "width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Width of the docked chat",
            "description": "The docked chat's width until the person drags it; what they drag to wins.",
            "default": 400
          },
          "width_step": {
            "type": "number",
            "minimum": 1e-6,
            "title": "An arrow key moves it",
            "description": "How far an arrow key moves the drag handle.",
            "default": 16
          },
          "min_page_width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Page kept beside the chat",
            "description": "The page keeps at least this much beside the docked chat.",
            "default": 360
          },
          "text_width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Widest column of her text",
            "description": "The widest column her text keeps to when expanded.",
            "default": 960
          },
          "window_width": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Width of her own window",
            "description": "The width her own window opens at (desktop).",
            "default": 520
          },
          "window_height": {
            "type": "number",
            "minimum": 1e-6,
            "title": "Height of her own window",
            "description": "The height her own window opens at (desktop).",
            "default": 760
          },
          "transition_ms": {
            "type": "integer",
            "minimum": 0,
            "maximum": 18446744073709551615,
            "title": "The chat moves in",
            "description": "Milliseconds the chat takes to move; none when the system asks for no animations.",
            "default": 200
          },
          "connect_timeout": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "Her socket says it is ready within",
            "description": "Seconds her socket may take to say it is ready before it counts as failed.",
            "default": 10
          },
          "retry_delay": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "First reconnect after",
            "description": "Seconds before her socket's first reconnect; each next one doubles, up to `retry_max_delay`.",
            "default": 1
          },
          "retry_max_delay": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "Longest wait between reconnects",
            "description": "The longest wait between reconnects.",
            "default": 16
          }
        },
        "additionalProperties": false,
        "title": "Moneypenny's chat",
        "description": "`[oo.chat]`: Moneypenny's chat, in logical pixels.",
        "default": {
          "min_width": 320,
          "max_width": 720,
          "width": 400,
          "width_step": 16,
          "min_page_width": 360,
          "text_width": 960,
          "window_width": 520,
          "window_height": 760,
          "transition_ms": 200,
          "connect_timeout": 10,
          "retry_delay": 1,
          "retry_max_delay": 16
        }
      },
      "routes": {
        "type": "object",
        "properties": {
          "text_bytes": {
            "type": "integer",
            "minimum": 1,
            "format": "bytes",
            "title": "Longest text filter in a link",
            "description": "Bytes of the `text` filter of a page link; a web URL or one of Moneypenny's links with a longer one is refused like any other bad link. Keep it equal to the `unex` record's `route_text_bytes`, or her links that one accepts and this one does not show as a refused view.",
            "default": 128
          }
        },
        "additionalProperties": false,
        "title": "Page links",
        "description": "`[oo.routes]`.",
        "default": {
          "text_bytes": 128
        }
      },
      "art": {
        "type": "object",
        "properties": {
          "max_width": {
            "type": "integer",
            "minimum": 1,
            "title": "Widest hero picture",
            "description": "Pixels of the widest hero picture ever fetched; a site on slow links sets 1920.",
            "default": 3840
          }
        },
        "additionalProperties": false,
        "title": "Hero pictures",
        "description": "`[oo.art]`.",
        "default": {
          "max_width": 3840
        }
      },
      "refresh": {
        "type": "object",
        "properties": {
          "sessions": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "The page shown",
            "description": "What the page shown displays from the board of Sessions, Stations and Missions.",
            "default": 3.0
          },
          "overview": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "The overview",
            "description": "The overview's recent activity.",
            "default": 10.0
          },
          "timeline": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "A timeline",
            "description": "An Operation's timeline.",
            "default": 10.0
          },
          "pools": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "The account pools",
            "description": "The account pools.",
            "default": 15.0
          },
          "slow": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "The bell and the search",
            "description": "What the bell and the search need and the page shown does not display: Sessions, Stations, held briefs, green ink requests and Mission proposals; `0` reads them with every refresh.",
            "default": 15.0
          },
          "profile": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "Who the person is",
            "description": "Who the person is and what they may do; `0` reads it only at sign-in and when the operator menu opens.",
            "default": 60.0
          },
          "hidden": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "While nothing can be seen",
            "description": "Everything, while the tab or window cannot be seen; `0` reads nothing until it can.",
            "default": 60.0
          },
          "silent_intervals": {
            "type": "integer",
            "minimum": 1,
            "title": "Intervals before a Station is silent",
            "description": "A Station that has not checked in for this many `sessions` intervals shows as silent.",
            "default": 3
          },
          "renew_before": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "Renew a sign-in before it ends",
            "description": "How long before the access token ends the app renews it, and before the sign-in itself ends the person is told; `0` does neither, and the sign-in ends with the token. A shorter lifetime than this is renewed halfway through.",
            "default": 600.0
          },
          "renew_retry": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "Retry a failed renewal after",
            "description": "How soon a failed renewal is tried again when `sessions` is off; else it is tried again at `sessions`.",
            "default": 30
          }
        },
        "additionalProperties": false,
        "title": "How often pages ask again",
        "description": "`[oo.refresh]`: how often each page asks again; `0` turns that refresh off.",
        "default": {
          "sessions": 3.0,
          "overview": 10.0,
          "timeline": 10.0,
          "pools": 15.0,
          "slow": 15.0,
          "profile": 60.0,
          "hidden": 60.0,
          "silent_intervals": 3,
          "renew_before": 600.0,
          "renew_retry": 30
        }
      },
      "controls": {
        "type": "object",
        "properties": {
          "wait": {
            "type": "number",
            "minimum": 0,
            "format": "seconds",
            "title": "Follow a control for",
            "description": "How long to follow a control for the Station's outcome; `0` returns once it is queued.",
            "default": 10.0
          },
          "poll": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "Ask for its status every",
            "description": "How often to ask for its status meanwhile.",
            "default": 0.5
          },
          "confirming": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "A proposal may stay applying",
            "description": "How long a proposal of Moneypenny's may stay applying before its card says the outcome is unknown.",
            "default": 30
          }
        },
        "additionalProperties": false,
        "title": "Following a control",
        "description": "`[oo.controls]`: following a Mission control until the Station reports.",
        "default": {
          "wait": 10.0,
          "poll": 0.5,
          "confirming": 30
        }
      },
      "pages": {
        "type": "object",
        "properties": {
          "timeline": {
            "type": "integer",
            "minimum": 1,
            "title": "Timeline items in a page",
            "description": "Items in one page of an Operation's timeline.",
            "default": 50
          },
          "audit": {
            "type": "integer",
            "minimum": 1,
            "title": "Audit records shown",
            "description": "Records in the audit log.",
            "default": 100
          },
          "summary": {
            "type": "integer",
            "minimum": 1,
            "title": "Items in a summary",
            "description": "Items of recent activity, Operations and Stations a summary lists.",
            "default": 5
          },
          "search": {
            "type": "integer",
            "minimum": 1,
            "title": "Search results listed",
            "description": "Results the top bar's search lists at once.",
            "default": 24
          }
        },
        "additionalProperties": false,
        "title": "How much a page asks for",
        "description": "`[oo.pages]`: how much a page asks for.",
        "default": {
          "timeline": 50,
          "audit": 100,
          "summary": 5,
          "search": 24
        }
      },
      "terminal": {
        "type": "object",
        "properties": {
          "scrollback_lines": {
            "type": "integer",
            "minimum": 1,
            "title": "Lines kept",
            "description": "Lines the terminal keeps.",
            "default": 10000
          },
          "connect_timeout": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "The socket opens within",
            "description": "How long the socket may take to open.",
            "default": 10
          },
          "retry_delay": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "First reconnect after",
            "description": "The wait before the first reconnect; each next one doubles, up to `retry_max_delay`.",
            "default": 1
          },
          "retry_max_delay": {
            "type": "number",
            "minimum": 1e-6,
            "format": "seconds",
            "title": "Longest wait between reconnects",
            "description": "The longest wait between reconnects.",
            "default": 16
          }
        },
        "additionalProperties": false,
        "title": "The live terminal",
        "description": "`[oo.terminal]`: the live Session terminal.",
        "default": {
          "scrollback_lines": 10000,
          "connect_timeout": 10,
          "retry_delay": 1,
          "retry_max_delay": 16
        }
      },
      "limits": {
        "type": "object",
        "properties": {
          "mission_title_chars": {
            "type": "integer",
            "minimum": 0,
            "maximum": 18446744073709551615,
            "title": "A new Mission's title",
            "description": "A new Mission's title.",
            "default": 512
          },
          "mission_prompt_chars": {
            "type": "integer",
            "minimum": 0,
            "maximum": 18446744073709551615,
            "title": "A new Mission's instructions",
            "description": "A new Mission's instructions.",
            "default": 65536
          },
          "reason_chars": {
            "type": "integer",
            "minimum": 0,
            "maximum": 18446744073709551615,
            "title": "The reason for lowering a clearance",
            "description": "The reason for lowering an Agent's clearance.",
            "default": 512
          }
        },
        "additionalProperties": false,
        "title": "How much a text field takes",
        "description": "`[oo.limits]`: how much a text field takes, in characters; `0` takes the bound away.",
        "default": {
          "mission_title_chars": 512,
          "mission_prompt_chars": 65536,
          "reason_chars": 512
        }
      },
      "assets": {
        "type": "object",
        "properties": {
          "cdn": {
            "type": "string",
            "enum": [
              "none",
              "fonts",
              "all"
            ],
            "x-enum-labels": {
              "none": "Nothing from outside",
              "fonts": "Fonts",
              "all": "Fonts and the engine"
            },
            "title": "Fetched from outside",
            "description": "What the web page may fetch from outside its own origin; the desktop app reads nothing here.",
            "default": "none"
          }
        },
        "additionalProperties": false,
        "title": "What a web page fetches",
        "description": "`[oo.assets]`: read by the web page's bootstrap, before the app runs.",
        "default": {
          "cdn": "none"
        }
      }
    },
    "additionalProperties": false
  },
  "form": {
    "fields": {
      "appearance": {
        "form": {}
      },
      "chat": {
        "form": {}
      },
      "routes": {
        "form": {}
      },
      "art": {
        "form": {}
      },
      "refresh": {
        "form": {}
      },
      "controls": {
        "form": {}
      },
      "pages": {
        "form": {}
      },
      "terminal": {
        "form": {}
      },
      "limits": {
        "form": {}
      },
      "assets": {
        "form": {}
      }
    }
  },
  "key": {
    "title": "Outer Office settings",
    "description": "",
    "fixed": "settings"
  }
}
''';
