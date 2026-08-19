import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';

const dataSeed = 20260818;
const listCount = 1000;
const updateCount = 500;
const updateTicks = 80;
const updateBatch = 50;
const tileCount = 64;

// Deterministic fake credential for the auth benchmark. Disclosed in the repo so
// Reactor can record and assert on it; exercising wrong-password and account-exists
// branches stays possible.
const demoUsername = 'test';
const demoPassword = 'test';

int deterministicValue(int index, [int tick = 0]) =>
    ((dataSeed + index * 1103515245 + tick * 2654435761) & 0xffffffff) %
    10000;

void main() {
  // Flutter renders to a single canvas and only builds its semantics tree when an accessibility
  // client asks for it, so by default an external driver sees an empty view hierarchy. A benchmark
  // app has to be inspectable, so semantics are forced on here.
  //
  // Disclosed deliberately: this is a measurable difference from a stock Flutter app. Native views
  // in the React Native build are always present, so enabling semantics moves the two builds closer
  // to parity rather than further apart — but it is not zero cost and must be stated.
  WidgetsFlutterBinding.ensureInitialized();
  SemanticsBinding.instance.ensureSemantics();
  runApp(const ReactorApp());
}

class ReactorApp extends StatelessWidget {
  const ReactorApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Reactor',
      debugShowCheckedModeBanner: false,
      themeMode: ThemeMode.system,
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xff5b5bd6)), useMaterial3: true),
      darkTheme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xff8b8cf8), brightness: Brightness.dark), useMaterial3: true),
      home: const RootGate(),
    );
  }
}

class RootGate extends StatefulWidget {
  const RootGate({super.key});
  @override
  State<RootGate> createState() => _RootGateState();
}

class _RootGateState extends State<RootGate> {
  String? session;

  @override
  Widget build(BuildContext context) {
    final current = session;
    if (current == null) {
      return AuthScenario(onSignedIn: (value) => setState(() => session = value));
    }
    return HomeScreen(session: current, onSignOut: () => setState(() => session = null));
  }
}

class HomeScreen extends StatefulWidget {
  const HomeScreen({required this.session, required this.onSignOut, super.key});
  final String session;
  final VoidCallback onSignOut;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  void open(Widget page) => Navigator.of(context).push(MaterialPageRoute(builder: (_) => page));

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisAlignment: MainAxisAlignment.center, children: [
            Text('Flutter · Release benchmark', style: Theme.of(context).textTheme.labelLarge?.copyWith(color: colors.primary)),
            const SizedBox(height: 6),
            Text('Reactor', style: Theme.of(context).textTheme.displayMedium?.copyWith(fontWeight: FontWeight.w800)),
            const SizedBox(height: 8),
            Text('Reactor ready', style: Theme.of(context).textTheme.bodyLarge?.copyWith(color: colors.onSurfaceVariant)),
            const SizedBox(height: 8),
            Text(widget.session, style: Theme.of(context).textTheme.bodyMedium?.copyWith(color: colors.primary, fontWeight: FontWeight.w700)),
            const SizedBox(height: 36),
            FilledButton(onPressed: () => open(const ListScenario()), child: const Text('List scenario')),
            const SizedBox(height: 12),
            FilledButton(onPressed: () => open(const UpdateScenario()), child: const Text('Update scenario')),
            const SizedBox(height: 12),
            FilledButton(onPressed: () => open(const AnimationScenario()), child: const Text('Animation scenario')),
            const SizedBox(height: 12),
            FilledButton(onPressed: widget.onSignOut, child: const Text('Sign out')),
            const SizedBox(height: 28),
            Text('Deterministic data · no network · optimized APIs', style: Theme.of(context).textTheme.bodySmall?.copyWith(color: colors.onSurfaceVariant)),
          ]),
        ),
      ),
    );
  }
}

class ListScenario extends StatelessWidget {
  const ListScenario({super.key});
  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('List ready')),
    body: ListView.builder(itemCount: listCount, itemExtent: 96, itemBuilder: (_, index) => BenchRow(index: index, value: deterministicValue(index))),
  );
}

class BenchRow extends StatelessWidget {
  const BenchRow({required this.index, required this.value, super.key});
  final int index;
  final int value;
  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
      child: DecoratedBox(
        decoration: BoxDecoration(color: colors.surfaceContainer, border: Border.all(color: colors.outlineVariant), borderRadius: BorderRadius.circular(14)),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Row(children: [
            Container(width: 44, height: 44, alignment: Alignment.center, decoration: BoxDecoration(color: colors.primary, borderRadius: BorderRadius.circular(12)), child: Text('${index % 100}', style: TextStyle(color: colors.onPrimary, fontWeight: FontWeight.w800))),
            const SizedBox(width: 12),
            Expanded(child: Column(mainAxisAlignment: MainAxisAlignment.center, crossAxisAlignment: CrossAxisAlignment.start, children: [Text('Item $index', style: const TextStyle(fontWeight: FontWeight.w700)), const SizedBox(height: 4), Text('Deterministic value $value', style: TextStyle(fontSize: 12, color: colors.onSurfaceVariant))])),
            Text('$value', style: TextStyle(color: colors.primary, fontWeight: FontWeight.w700)),
          ]),
        ),
      ),
    );
  }
}

class UpdateScenario extends StatefulWidget {
  const UpdateScenario({super.key});
  @override
  State<UpdateScenario> createState() => _UpdateScenarioState();
}

class _UpdateScenarioState extends State<UpdateScenario> {
  late List<int> values;
  Timer? timer;
  int tick = 0;
  bool complete = false;

  @override
  void initState() {
    super.initState();
    values = List.generate(updateCount, deterministicValue);
    timer = Timer.periodic(const Duration(milliseconds: 100), (_) {
      final nextTick = tick + 1;
      final next = List<int>.of(values);
      for (var offset = 0; offset < updateBatch; offset++) {
        final index = (nextTick * updateBatch + offset * 7) % updateCount;
        next[index] = deterministicValue(index, nextTick);
      }
      setState(() { tick = nextTick; values = next; complete = tick >= updateTicks; });
      if (complete) timer?.cancel();
    });
  }

  @override
  void dispose() { timer?.cancel(); super.dispose(); }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Update ready')),
    body: Column(children: [
      SizedBox(height: 40, child: Center(child: Text(complete ? 'Update complete' : 'Updating · tick $tick', style: const TextStyle(fontWeight: FontWeight.w700)))),
      Expanded(child: ListView.builder(itemCount: values.length, itemExtent: 96, itemBuilder: (_, index) => BenchRow(index: index, value: values[index]))),
    ]),
  );
}

class AnimationScenario extends StatefulWidget {
  const AnimationScenario({super.key});
  @override
  State<AnimationScenario> createState() => _AnimationScenarioState();
}

class AuthScenario extends StatefulWidget {
  const AuthScenario({required this.onSignedIn, super.key});
  final ValueChanged<String> onSignedIn;
  @override
  State<AuthScenario> createState() => _AuthScenarioState();
}

class _AuthScenarioState extends State<AuthScenario> {
  bool signIn = true;
  final username = TextEditingController();
  final password = TextEditingController();
  final confirm = TextEditingController();
  String? error;

  @override
  void dispose() {
    username.dispose();
    password.dispose();
    confirm.dispose();
    super.dispose();
  }

  void submit() {
    setState(() { error = null; });
    if (signIn) {
      if (username.text == demoUsername && password.text == demoPassword) {
        widget.onSignedIn('Signed in as ${username.text}');
      } else {
        setState(() { error = 'Invalid username or password'; });
      }
      return;
    }
    if (username.text.isEmpty) { setState(() { error = 'Username required'; }); return; }
    if (username.text == demoUsername) { setState(() { error = 'Account already exists'; }); return; }
    if (password.text != confirm.text) { setState(() { error = 'Passwords do not match'; }); return; }
    widget.onSignedIn('Account created as ${username.text}');
  }

  @override
  Widget build(BuildContext context) {
    final title = signIn ? 'Sign in' : 'Sign up';
    final colors = Theme.of(context).colorScheme;
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisAlignment: MainAxisAlignment.center, children: [
            Text('Reactor',
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.displaySmall?.copyWith(fontWeight: FontWeight.w800)),
            Text(title,
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.titleLarge?.copyWith(color: colors.primary)),
            const SizedBox(height: 24),
            SegmentedButton<bool>(
              segments: const [
                ButtonSegment(value: true, label: Text('Sign in')),
                ButtonSegment(value: false, label: Text('Sign up')),
              ],
              selected: {signIn},
              onSelectionChanged: (selection) => setState(() { signIn = selection.first; error = null; }),
            ),
            const SizedBox(height: 16),
            TextField(
              controller: username,
              autofillHints: const [AutofillHints.username],
              decoration: const InputDecoration(labelText: 'Username', border: OutlineInputBorder()),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: password,
              obscureText: true,
              decoration: const InputDecoration(labelText: 'Password', border: OutlineInputBorder()),
            ),
            if (!signIn) ...[
              const SizedBox(height: 12),
              TextField(
                controller: confirm,
                obscureText: true,
                decoration: const InputDecoration(labelText: 'Confirm password', border: OutlineInputBorder()),
              ),
            ],
            if (error != null) ...[
              const SizedBox(height: 12),
              Text(error!, style: const TextStyle(color: Colors.red, fontWeight: FontWeight.w600)),
            ],
            const SizedBox(height: 16),
            FilledButton(onPressed: submit, child: Text(signIn ? 'Sign in' : 'Create account')),
          ]),
        ),
      ),
    );
  }
}
class _AnimationScenarioState extends State<AnimationScenario> with SingleTickerProviderStateMixin {
  late final AnimationController controller;
  Timer? timer;
  bool complete = false;
  @override
  void initState() {
    super.initState();
    controller = AnimationController(vsync: this, duration: const Duration(milliseconds: 800))..repeat(reverse: true);
    timer = Timer(const Duration(seconds: 8), () { controller.stop(); setState(() => complete = true); });
  }
  @override
  void dispose() { timer?.cancel(); controller.dispose(); super.dispose(); }
  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Scaffold(
      appBar: AppBar(title: const Text('Animation ready')),
      body: Column(children: [
        SizedBox(height: 48, child: Center(child: Text(complete ? 'Animation complete' : 'Animating 64 tiles', style: const TextStyle(fontWeight: FontWeight.w700)))),
        AnimatedBuilder(animation: controller, builder: (_, __) => Wrap(spacing: 10, runSpacing: 10, children: List.generate(tileCount, (index) => Opacity(opacity: 0.55 + controller.value * 0.45, child: Transform.translate(offset: Offset(0, -10 + controller.value * 20), child: Container(width: 30, height: 30, decoration: BoxDecoration(color: index.isEven ? colors.primary : colors.tertiary, borderRadius: BorderRadius.circular(8)))))))),
      ]),
    );
  }
}
