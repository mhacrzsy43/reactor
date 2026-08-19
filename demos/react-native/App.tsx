import React, {useEffect, useMemo, useRef, useState} from 'react';
import {
  Animated,
  FlatList,
  Pressable,
  StatusBar,
  StyleSheet,
  Text,
  useColorScheme,
  View,
} from 'react-native';
import {
  SafeAreaProvider,
  useSafeAreaInsets,
} from 'react-native-safe-area-context';

type Screen = 'home' | 'list' | 'update' | 'animation';
type Palette = ReturnType<typeof paletteFor>;

const DATA_SEED = 20260818;
const LIST_COUNT = 1000;
const UPDATE_COUNT = 500;
const UPDATE_TICKS = 80;
const UPDATE_BATCH = 50;
const TILE_COUNT = 64;

function paletteFor(dark: boolean) {
  return dark
    ? {background: '#0d1117', surface: '#171c24', text: '#f3f4f6', muted: '#9ca3af', line: '#2d3542', accent: '#8b8cf8', accentText: '#0d1026', tile: '#5f60d7'}
    : {background: '#f4f6fb', surface: '#ffffff', text: '#18202f', muted: '#667085', line: '#e4e7ec', accent: '#5b5bd6', accentText: '#ffffff', tile: '#8585ef'};
}

function deterministicValue(index: number, tick = 0) {
  return ((DATA_SEED + index * 1103515245 + tick * 2654435761) >>> 0) % 10000;
}

function App() {
  const dark = useColorScheme() === 'dark';
  const palette = useMemo(() => paletteFor(dark), [dark]);
  return (
    <SafeAreaProvider>
      <StatusBar barStyle={dark ? 'light-content' : 'dark-content'} backgroundColor={palette.background} />
      <BenchApp palette={palette} />
    </SafeAreaProvider>
  );
}

function BenchApp({palette}: {palette: Palette}) {
  const insets = useSafeAreaInsets();
  const [screen, setScreen] = useState<Screen>('home');
  const common = {palette, onBack: () => setScreen('home')};
  return (
    <View style={[styles.root, {paddingTop: insets.top, paddingBottom: insets.bottom, backgroundColor: palette.background}]}>
      {screen === 'home' && <Home palette={palette} onSelect={setScreen} />}
      {screen === 'list' && <ListScenario {...common} />}
      {screen === 'update' && <UpdateScenario {...common} />}
      {screen === 'animation' && <AnimationScenario {...common} />}
    </View>
  );
}

function Header({title, palette, onBack}: {title: string; palette: Palette; onBack: () => void}) {
  return (
    <View style={[styles.header, {borderBottomColor: palette.line}]}>
      <Pressable accessibilityRole="button" onPress={onBack} style={styles.backButton}><Text style={[styles.backText, {color: palette.accent}]}>Back</Text></Pressable>
      <Text style={[styles.headerTitle, {color: palette.text}]}>{title}</Text>
      <View style={styles.backButton} />
    </View>
  );
}

function Home({palette, onSelect}: {palette: Palette; onSelect: (screen: Screen) => void}) {
  return (
    <View style={styles.home}>
      <Text style={[styles.eyebrow, {color: palette.accent}]}>React Native · Release benchmark</Text>
      <Text style={[styles.title, {color: palette.text}]}>Reactor</Text>
      <Text accessibilityRole="text" style={[styles.ready, {color: palette.muted}]}>Reactor ready</Text>
      <View style={styles.buttonStack}>
        <BenchButton text="List scenario" palette={palette} onPress={() => onSelect('list')} />
        <BenchButton text="Update scenario" palette={palette} onPress={() => onSelect('update')} />
        <BenchButton text="Animation scenario" palette={palette} onPress={() => onSelect('animation')} />
      </View>
      <Text style={[styles.caption, {color: palette.muted}]}>Deterministic data · no network · optimized APIs</Text>
    </View>
  );
}

function BenchButton({text, palette, onPress}: {text: string; palette: Palette; onPress: () => void}) {
  return (
    <Pressable accessibilityRole="button" onPress={onPress} style={({pressed}) => [styles.button, {backgroundColor: palette.accent, opacity: pressed ? 0.82 : 1}]}>
      <Text style={[styles.buttonText, {color: palette.accentText}]}>{text}</Text>
    </Pressable>
  );
}

function ListScenario({palette, onBack}: {palette: Palette; onBack: () => void}) {
  const data = useMemo(() => Array.from({length: LIST_COUNT}, (_, index) => index), []);
  return (
    <View style={styles.fill}>
      <Header title="List ready" palette={palette} onBack={onBack} />
      <FlatList
        data={data}
        keyExtractor={item => String(item)}
        getItemLayout={(_, index) => ({length: 96, offset: 96 * index, index})}
        initialNumToRender={12}
        windowSize={7}
        removeClippedSubviews
        renderItem={({item}) => <BenchRow index={item} value={deterministicValue(item)} palette={palette} />}
      />
    </View>
  );
}

function BenchRow({index, value, palette}: {index: number; value: number; palette: Palette}) {
  return (
    <View style={[styles.row, {backgroundColor: palette.surface, borderColor: palette.line}]}>
      <View style={[styles.avatar, {backgroundColor: palette.tile}]}><Text style={styles.avatarText}>{index % 100}</Text></View>
      <View style={styles.rowCopy}><Text style={[styles.rowTitle, {color: palette.text}]}>Item {index}</Text><Text style={[styles.rowMeta, {color: palette.muted}]}>Deterministic value {value}</Text></View>
      <Text style={[styles.rowValue, {color: palette.accent}]}>{value}</Text>
    </View>
  );
}

function UpdateScenario({palette, onBack}: {palette: Palette; onBack: () => void}) {
  const [values, setValues] = useState(() => Array.from({length: UPDATE_COUNT}, (_, index) => deterministicValue(index)));
  const [tick, setTick] = useState(0);
  const [complete, setComplete] = useState(false);

  useEffect(() => {
    const timer = setInterval(() => {
      setTick(currentTick => {
        const nextTick = currentTick + 1;
        setValues(current => {
          const next = [...current];
          for (let offset = 0; offset < UPDATE_BATCH; offset += 1) {
            const index = (nextTick * UPDATE_BATCH + offset * 7) % UPDATE_COUNT;
            next[index] = deterministicValue(index, nextTick);
          }
          return next;
        });
        if (nextTick >= UPDATE_TICKS) {
          clearInterval(timer);
          setComplete(true);
        }
        return nextTick;
      });
    }, 100);
    return () => clearInterval(timer);
  }, []);

  return (
    <View style={styles.fill}>
      <Header title="Update ready" palette={palette} onBack={onBack} />
      <Text style={[styles.status, {color: complete ? palette.accent : palette.muted}]}>{complete ? 'Update complete' : `Updating · tick ${tick}`}</Text>
      <FlatList data={values} extraData={tick} keyExtractor={(_, index) => String(index)} getItemLayout={(_, index) => ({length: 96, offset: 96 * index, index})} initialNumToRender={12} windowSize={7} removeClippedSubviews renderItem={({item, index}) => <BenchRow index={index} value={item} palette={palette} />} />
    </View>
  );
}

function AnimationScenario({palette, onBack}: {palette: Palette; onBack: () => void}) {
  const progress = useRef(new Animated.Value(0)).current;
  const [complete, setComplete] = useState(false);
  useEffect(() => {
    const loop = Animated.loop(Animated.sequence([
      Animated.timing(progress, {toValue: 1, duration: 400, useNativeDriver: true}),
      Animated.timing(progress, {toValue: 0, duration: 400, useNativeDriver: true}),
    ]));
    loop.start();
    const timer = setTimeout(() => { loop.stop(); setComplete(true); }, 8000);
    return () => { clearTimeout(timer); loop.stop(); };
  }, [progress]);
  const translateY = progress.interpolate({inputRange: [0, 1], outputRange: [-10, 10]});
  const opacity = progress.interpolate({inputRange: [0, 1], outputRange: [0.55, 1]});
  return (
    <View style={styles.fill}>
      <Header title="Animation ready" palette={palette} onBack={onBack} />
      <Text style={[styles.status, {color: complete ? palette.accent : palette.muted}]}>{complete ? 'Animation complete' : 'Animating 64 tiles'}</Text>
      <View style={styles.tileGrid}>{Array.from({length: TILE_COUNT}, (_, index) => <Animated.View key={index} style={[styles.tile, {backgroundColor: index % 2 ? palette.accent : palette.tile, opacity, transform: [{translateY}]}]} />)}</View>
    </View>
  );
}

const styles = StyleSheet.create({
  root: {flex: 1}, fill: {flex: 1}, home: {flex: 1, paddingHorizontal: 24, justifyContent: 'center'}, eyebrow: {fontSize: 13, fontWeight: '700', letterSpacing: 0.5}, title: {fontSize: 42, lineHeight: 50, fontWeight: '800', marginTop: 6}, ready: {fontSize: 16, marginTop: 8}, buttonStack: {gap: 12, marginTop: 36}, button: {height: 54, borderRadius: 14, alignItems: 'center', justifyContent: 'center'}, buttonText: {fontSize: 16, fontWeight: '700'}, caption: {fontSize: 13, marginTop: 28},
  header: {height: 56, flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth, paddingHorizontal: 12}, backButton: {width: 72, paddingVertical: 12}, backText: {fontSize: 15, fontWeight: '700'}, headerTitle: {flex: 1, textAlign: 'center', fontSize: 17, fontWeight: '700'},
  row: {height: 88, marginHorizontal: 12, marginTop: 8, borderWidth: 1, borderRadius: 14, padding: 12, flexDirection: 'row', alignItems: 'center'}, avatar: {width: 44, height: 44, borderRadius: 12, alignItems: 'center', justifyContent: 'center'}, avatarText: {color: '#ffffff', fontSize: 13, fontWeight: '800'}, rowCopy: {flex: 1, marginLeft: 12}, rowTitle: {fontSize: 15, fontWeight: '700'}, rowMeta: {fontSize: 12, marginTop: 4}, rowValue: {fontSize: 14, fontVariant: ['tabular-nums'], fontWeight: '700'}, status: {height: 40, paddingHorizontal: 16, textAlignVertical: 'center', paddingTop: 10, fontSize: 14, fontWeight: '700'},
  tileGrid: {padding: 20, flexDirection: 'row', flexWrap: 'wrap', gap: 10}, tile: {width: 30, height: 30, borderRadius: 8},
});

export default App;
