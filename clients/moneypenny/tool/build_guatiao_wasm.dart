import 'dart:convert';
import 'dart:io';

Future<void> main() async {
  final packageConfigFile = File('.dart_tool/package_config.json').absolute;
  if (!packageConfigFile.existsSync()) {
    stderr.writeln('Run flutter pub get before preparing the web build.');
    exitCode = 1;
    return;
  }

  final config = jsonDecode(
    await packageConfigFile.readAsString(),
  ) as Map<String, dynamic>;
  final packages = config['packages'] as List<dynamic>;
  final guatiaoPackage = packages.cast<Map<String, dynamic>>().firstWhere(
    (package) => package['name'] == 'guatiao',
    orElse: () => <String, dynamic>{},
  );
  if (guatiaoPackage.isEmpty) {
    stderr.writeln('The guatiao Dart package is not in package_config.json.');
    exitCode = 1;
    return;
  }

  final packageUri = packageConfigFile.uri.resolve(
    guatiaoPackage['rootUri'] as String,
  );
  var directory = Directory.fromUri(packageUri);
  Directory? repository;
  for (var depth = 0; depth < 5; depth++) {
    if (File('${directory.path}/Cargo.toml').existsSync() &&
        File('${directory.path}/crates/guatiao-intake/Cargo.toml')
            .existsSync()) {
      repository = directory;
      break;
    }
    final parent = directory.parent;
    if (parent.path == directory.path) break;
    directory = parent;
  }
  if (repository == null) {
    stderr.writeln('Could not locate the Guatiao Cargo workspace.');
    exitCode = 1;
    return;
  }

  final manifest = '${repository.path}/Cargo.toml';
  final metadata = await Process.run('cargo', [
    'metadata',
    '--manifest-path',
    manifest,
    '--format-version',
    '1',
  ], workingDirectory: repository.path);
  if (metadata.exitCode != 0) {
    stderr.writeln('Could not read the Guatiao Cargo workspace metadata.');
    exitCode = metadata.exitCode;
    return;
  }
  final targetDirectory =
      (jsonDecode(metadata.stdout as String)
              as Map<String, dynamic>)['target_directory']
          as String;
  final result = await Process.start('cargo', [
    'build',
    '--manifest-path',
    manifest,
    '-p',
    'guatiao-intake',
    '--target',
    'wasm32-unknown-unknown',
    '--release',
  ], mode: ProcessStartMode.inheritStdio);
  final buildExitCode = await result.exitCode;
  if (buildExitCode != 0) {
    exitCode = buildExitCode;
    return;
  }

  final wasm = File(
    '$targetDirectory/wasm32-unknown-unknown/release/guatiao_intake.wasm',
  );
  if (!wasm.existsSync()) {
    stderr.writeln('Cargo succeeded without producing guatiao_intake.wasm.');
    exitCode = 1;
    return;
  }
  final webDirectory = Directory('web');
  if (!webDirectory.existsSync()) await webDirectory.create(recursive: true);
  await wasm.copy('web/ai6_guatiao.wasm');
  stdout.writeln('Prepared the Guatiao WebAssembly protocol module.');
}
