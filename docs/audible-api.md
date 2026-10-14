# Audible API: observed behaviour

Requests and responses observed while building tbard (October 2026, `us`
marketplace, `api.audible.com`). This is the reference tbard is implemented
from: the protocol as seen on the wire, not third-party code. No tokens,
vouchers or account data are included.

Test books: *Juramentada* (`B09GX6H7J8`, 151 chapters, 57 h 39 min) and
*Robot salvaje* (`B0DKXZLL89`, 86 chapters, 4 h 25 min).

## Signing requests ✅ confirmed

With a device registered as an iPhone (`A2CZJZGLK2JJVM`), `Authorization:
Bearer` + `client-id: 0` fails with `400 "The specified authorization token
does not correspond to the specified Client-ID"`. What works is an **ADP
signature** on every request:

- `x-adp-token: <adp_token>`
- `x-adp-alg: SHA256withRSA:1.0`
- `x-adp-signature: <base64(signature)>:<date>`
  - date: UTC `YYYY-MM-DDTHH:MM:SS.ffffffZ`
  - signature: RSA PKCS#1 v1.5 + SHA-256 with `device_private_key` (PKCS#1
    PEM) over `METHOD\nPATH?QUERY\nDATE\nBODY\nADP_TOKEN`

The API never needs a refreshed `access_token`; only device deregistration
does (see below).

## Device registration ✅ confirmed

1. Open `https://www.amazon.<tld>/ap/signin` with OAuth + PKCE parameters
   (`openid.oa2.code_challenge`, `openid.oa2.client_id=device:<hex(serial#type)>`,
   `openid.assoc_handle=amzn_audible_ios_<country>`, `marketPlaceId`, …).
2. After signing in, the browser lands on `…/ap/maplanding?…` carrying
   `openid.oa2.authorization_code`.
3. `POST https://api.amazon.<tld>/auth/register` with the code and the PKCE
   verifier returns `bearer` tokens, `mac_dms` (`adp_token`,
   `device_private_key`), `device_info` and `customer_info`.

## Device deregistration (implemented, not yet tested on a real account)

1. `POST https://api.amazon.<tld>/auth/token` (form: `source_token=<refresh_token>`,
   `source_token_type=refresh_token`, `requested_token_type=access_token`,
   `app_name=Audible`) → `access_token`.
2. `POST https://api.amazon.<tld>/auth/deregister` with
   `Authorization: Bearer <access_token>` and
   `{"deregister_all_existing_accounts": false}`.

## Reading the position

`GET 1.0/annotations/lastpositions?asins=<ASIN>[,<ASIN>…]`

```json
{"asin_last_position_heard_annots": [{"asin": "B09GX6H7J8",
  "last_position_heard": {"last_updated": "2026-08-14 21:17:37.114",
                          "position_ms": 6985777, "status": "Exists"}}],
 "response_groups": ["always-returned"]}
```

- `last_updated` is UTC without a zone, `YYYY-MM-DD HH:MM:SS.mmm`, and may
  have fewer than three decimals (`02:59:17.94`).

## Writing the position (Whispersync) ✅ confirmed

`PUT 1.0/lastpositions/<ASIN>`

```json
{"acr": "<acr>", "asin": "<ASIN>", "position_ms": 7200000}
```

- Answers `200` with an empty body.
- A read right after returns the new position and `last_updated` = now.
- The phone app showed the new position (2:00:00) after the PUT.
- `acr` is required. It comes from `content_reference.acr` in the metadata
  or the license request and is stable per content version, so tbard stores it.

## Metadata (chapters, acr, last position)

`GET 1.0/content/<ASIN>/metadata?response_groups=last_position_heard,content_reference,chapter_info`

- `content_metadata.content_reference`: `acr`, `asin`, `codec` (`mp4a.40.2`),
  `content_format`, `content_size_in_bytes`, `marketplace`, `sku`, `version`.
- `content_metadata.chapter_info`: `brandIntroDurationMs`,
  `brandOutroDurationMs`, `is_accurate`, `runtime_length_ms`, `chapters[]`.
  - Each chapter: `{title, start_offset_ms, start_offset_sec, length_ms}`,
    possibly with nested `chapters[]` (parts or books in a bundle).
  - Titles can be generic ("Capítulo 8") or real ("Capítulo 31: La Primera
    Noche"), depending on the book.

## Available codecs

`GET 1.0/library/<ASIN>?response_groups=media,product_attrs` → `item.available_codecs[]`:

| name | enhanced_codec |
|---|---|
| `aax_22_64` | `LC_64_22050_stereo` |
| `aax_22_32` | `LC_32_22050_stereo` |
| `aax` | `aax` |

The bitrate is in `enhanced_codec` (`LC_<kbps>_<Hz>_<channels>`).

## License and download

`POST 1.0/content/<ASIN>/licenserequest`

```json
{"consumption_type": "Download", "supported_drm_types": ["Adrm", "Mpeg"],
 "quality": "High", "response_groups": "content_reference,chapter_info"}
```

| quality | granted content_format | size | kbps |
|---|---|---|---|
| `High` | `AAX_22_64` | 1 678 137 467 B | 64 |
| `Normal` | `AAX_22_32` | 847 875 394 B | 32 |

- `High` grants the best entry of `available_codecs`, so tbard always asks
  for `High` (*Robot salvaje* got `AAX_44_128`: AAC 44.1 kHz, ~126 kbps).
- `content_license`: `status_code` (`Granted`), `drm_type` (`Adrm`),
  `license_response` (encrypted voucher), `content_metadata.content_url.offline_url`
  (a signed CDN URL), `content_metadata.content_reference`,
  `content_metadata.chapter_info`.
- The voucher decrypts with AES-128-CBC; key = bytes 0..16 and IV = bytes
  16..32 of SHA-256(`device_type + device_serial + user_id + asin`). The
  resulting JSON has `key` and `iv` in hex for `ffmpeg -audible_key/-audible_iv`.

## Splitting by chapter ✅ confirmed

- Cutting the AAXC per chapter with `-ss/-t` before `-i` and `-c copy` works
  without an intermediate file: 86 chapters in about 5 s, at most 30 ms off
  per chapter and 0.9 s over the whole book.
- The book-wide position (chapter `start_offset_ms` + position inside its
  file) matches the phone app: the same chapter and minute in tbard and in
  the app. The brand intro is part of both the file and the offsets.

## Still to check

- Whether there is an endpoint to mark a book as finished; tbard sends the
  final position instead.
