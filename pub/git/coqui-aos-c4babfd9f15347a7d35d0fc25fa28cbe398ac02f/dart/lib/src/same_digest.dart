/// Whether two digests in text are the same digest.
library;

import 'digest.dart';
import 'error.dart';

/// Whether [a] and [b] are the same digest; one that does not parse is no digest.
bool sameDigest(String a, String b) {
  try {
    return Digest.parse(a).toString() == Digest.parse(b).toString();
  } on AosError {
    return false;
  }
}
