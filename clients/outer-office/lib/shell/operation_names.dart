/// What each Operation is called, as the gateway says it for the signed-in account.
///
/// An Operation is addressed by its ID everywhere; a person reads its name. [of] gives the
/// name, and the ID when the Operation has none or is not known.
class OperationNames {
  OperationNames._();

  static Map<String, String> _names = const {};

  /// Replaces what is known with [names], by Operation ID.
  static void use(Map<String, String> names) =>
      _names = Map.unmodifiable(names);

  /// The name of Operation [id], or [id] when it has none.
  static String of(String id) => _names[id] ?? id;
}
