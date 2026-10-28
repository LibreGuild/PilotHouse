use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

// Icons PilotHouse uses beyond gpui-component's default set. Each one costs a
// couple of KB of binary size, so add names here only when a view uses them.
icon_assets!(
    ExtraIcons,
    [
        MousePointer2,
        Square,
        MapPin,
        Pin,
        PinOff,
        Undo2,
        GitCompareArrows,
        RotateCw,
        Globe,
        TabletSmartphone,
        Play,
        Plug,
        FolderDown,
        Package,
        House,
        CircleStop,
        SquareStop,
        Network,
        Brain,
        GitPullRequest,
        Orbit,
        RotateCcwClock,
        CalendarClock,
        Wrench,
        FlaskConical,
        Gauge,
        Braces,
        Container,
        Database,
        FileSpreadsheet,
        FileArchive,
        FileLock,
        FileCog,
        FileTerminal,
        FileImage,
        FileText,
        Check,
        Sparkle,
        Hexagon,
        Asterisk,
        Info,
        Keyboard,
        FolderOpen,
        LayoutGrid,
        FileCode,
        Palette,
        Camera,
        Crosshair,
        Eraser,
        Image,
        KeyRound,
        MoveUpRight,
        Pencil,
        Power,
        ScrollText,
        Settings2,
        ShieldCheck,
        Smartphone,
        Video,
        Bot,
        Circle,
        Files,
        FolderPlus,
        FilePlus,
        ChevronsDownUp,
        Volume2,
        VolumeOff,
        FolderInput,
        GitBranch,
        GitCompare,
        ListTodo,
        MessageSquare,
        SendHorizontal,
        Sparkles,
        SquareKanban,
        SquarePen,
        Terminal,
        Trash,
        Zap,
        ArrowDownToLine,
        ChevronsLeft,
        Clock,
        GripVertical,
        Layers,
        LayoutPanelTop,
        ListPlus,
        Pointer,
        Wifi,
        WifiOff,
    ]
);

/// The app's asset source: PilotHouse's extra icons layered over the
/// gpui-component defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct PilotHouseAssets;

impl AssetSource for PilotHouseAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
