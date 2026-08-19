import { useCallback, useEffect, useMemo, useState } from '@lynx-js/react'
import './App.css'

type Screen = 'home' | 'list' | 'update' | 'animation'
const DATA_SEED = 20260818
const LIST_COUNT = 1000
const UPDATE_COUNT = 500
const UPDATE_TICKS = 80
const UPDATE_BATCH = 50
const TILE_COUNT = 64

function deterministicValue(index: number, tick = 0) {
  return ((DATA_SEED + index * 1103515245 + tick * 2654435761) >>> 0) % 10000
}

export function App(props: { onMounted?: () => void }) {
  const [screen, setScreen] = useState<Screen>('home')
  useEffect(() => props.onMounted?.(), [props])
  if (screen === 'list') return <ListScenario onBack={() => setScreen('home')} />
  if (screen === 'update') return <UpdateScenario onBack={() => setScreen('home')} />
  if (screen === 'animation') return <AnimationScenario onBack={() => setScreen('home')} />
  return (
    <view className="page home" accessibility-element={false}>
      <text className="eyebrow">Lynx · Release benchmark</text>
      <text className="title">Reactor</text>
      <text
        className="muted ready"
        id="reactor-ready"
        accessibility-element={true}
        accessibility-label="Reactor ready"
        accessibility-traits="text"
      >
        Reactor ready
      </text>
      <view className="button-stack" accessibility-element={false}>
        <BenchButton automationId="list-scenario" text="List scenario" onTap={() => setScreen('list')} />
        <BenchButton automationId="update-scenario" text="Update scenario" onTap={() => setScreen('update')} />
        <BenchButton automationId="animation-scenario" text="Animation scenario" onTap={() => setScreen('animation')} />
      </view>
      <text className="muted caption">Deterministic data · no network · optimized APIs</text>
    </view>
  )
}

function BenchButton({ automationId, text, onTap }: { automationId: string; text: string; onTap: () => void }) {
  return (
    <view
      className="button"
      id={automationId}
      bindtap={onTap}
      accessibility-element={true}
      accessibility-label={text}
      accessibility-traits="button"
    >
      <text className="button-text">{text}</text>
    </view>
  )
}

function Header({ automationId, title, onBack }: { automationId: string; title: string; onBack: () => void }) {
  return (
    <view className="header" accessibility-element={false}>
      <text
        className="back"
        bindtap={onBack}
        accessibility-element={true}
        accessibility-label="Back"
        accessibility-traits="button"
      >
        Back
      </text>
      <text
        className="header-title"
        id={automationId}
        accessibility-element={true}
        accessibility-label={title}
        accessibility-traits="header"
      >
        {title}
      </text>
      <view className="header-spacer" accessibility-element={false} />
    </view>
  )
}

function Row({ index, value }: { index: number; value: number }) {
  return <view className="row" accessibility-element={false}><view className="avatar" accessibility-element={false}><text className="avatar-text">{index % 100}</text></view><view className="row-copy" accessibility-element={false}><text className="row-title">Item {index}</text><text className="row-meta">Deterministic value {value}</text></view><text className="row-value">{value}</text></view>
}

function ListScenario({ onBack }: { onBack: () => void }) {
  const data = useMemo(() => Array.from({ length: LIST_COUNT }, (_, index) => index), [])
  return <view className="page" accessibility-element={false}><Header automationId="list-ready" title="List ready" onBack={onBack} /><list className="list" list-type="single" span-count={1} scroll-orientation="vertical">{data.map(index => <list-item key={`item-${index}`} item-key={`item-${index}`} estimated-main-axis-size-px={96}><Row index={index} value={deterministicValue(index)} /></list-item>)}</list></view>
}

function UpdateScenario({ onBack }: { onBack: () => void }) {
  const [values, setValues] = useState(() => Array.from({ length: UPDATE_COUNT }, (_, index) => deterministicValue(index)))
  const [tick, setTick] = useState(0)
  const [complete, setComplete] = useState(false)
  useEffect(() => {
    const timer = setInterval(() => {
      setTick(current => {
        const nextTick = current + 1
        setValues(previous => {
          const next = [...previous]
          for (let offset = 0; offset < UPDATE_BATCH; offset += 1) {
            const index = (nextTick * UPDATE_BATCH + offset * 7) % UPDATE_COUNT
            next[index] = deterministicValue(index, nextTick)
          }
          return next
        })
        if (nextTick >= UPDATE_TICKS) { clearInterval(timer); setComplete(true) }
        return nextTick
      })
    }, 100)
    return () => clearInterval(timer)
  }, [])
  const status = complete ? 'Update complete' : `Updating · tick ${tick}`
  return <view className="page" accessibility-element={false}><Header automationId="update-ready" title="Update ready" onBack={onBack} /><text className="status" id={complete ? 'update-complete' : 'update-running'} accessibility-element={true} accessibility-label={status} accessibility-traits="updating">{status}</text><list className="list" list-type="single" span-count={1} scroll-orientation="vertical" layout-id={tick}>{values.map((value, index) => <list-item key={`update-${index}`} item-key={`update-${index}`} estimated-main-axis-size-px={96}><Row index={index} value={value} /></list-item>)}</list></view>
}

function AnimationScenario({ onBack }: { onBack: () => void }) {
  const [complete, setComplete] = useState(false)
  useEffect(() => { const timer = setTimeout(() => setComplete(true), 8000); return () => clearTimeout(timer) }, [])
  const stopClass = complete ? ' stopped' : ''
  const status = complete ? 'Animation complete' : 'Animating 64 tiles'
  return <view className="page" accessibility-element={false}><Header automationId="animation-ready" title="Animation ready" onBack={onBack} /><text className="status" id={complete ? 'animation-complete' : 'animation-running'} accessibility-element={true} accessibility-label={status} accessibility-traits="updating">{status}</text><view className="tile-grid" accessibility-element={false}>{Array.from({ length: TILE_COUNT }, (_, index) => <view key={index} className={`tile tile-${index % 2}${stopClass}`} accessibility-element={false} />)}</view></view>
}
