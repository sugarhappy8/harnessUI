use super::*;
use crate::fit::{self, COMPUTE_BASE_BYTES, FitInput};
use crate::layers::LayerSize;

const GB: u64 = 1_000_000_000;

fn sizes(input: u64, output: u64, layers: &[(u64, u64)], other: u64) -> LayerSizes {
    LayerSizes {
        input_bytes: input,
        output_bytes: output,
        layers: layers
            .iter()
            .map(|&(dense_bytes, expert_bytes)| LayerSize {
                dense_bytes,
                expert_bytes,
            })
            .collect(),
        other_bytes: other,
    }
}

/// A dense model: input 100, output 50, blocks of 10, 20, 30 and 40 (5 units).
fn dense() -> LayerSizes {
    sizes(100, 50, &[(10, 0), (20, 0), (30, 0), (40, 0)], 0)
}

/// A mixture of experts with blocks of different weights, to catch index mistakes.
fn uneven_experts() -> LayerSizes {
    sizes(1000, 50, &[(1, 100), (2, 200), (3, 300), (4, 400)], 5)
}

/// A mixture of experts with six equal blocks: dense 30, experts 50; output 20.
fn even_experts() -> LayerSizes {
    sizes(1000, 20, &[(30, 50); 6], 0)
}

fn input(
    sizes: &LayerSizes,
    kv: Vec<u64>,
    compute_gpu: u64,
    compute_cpu: u64,
    projector: u64,
) -> PlacementInput<'_> {
    PlacementInput {
        sizes,
        kv_per_layer: kv,
        compute_gpu,
        compute_cpu,
        projector,
    }
}

fn on_card(layers: Layers, experts_on_cpu: ExpertsPick) -> Where {
    Where {
        graphics: true,
        layers,
        experts_on_cpu,
        kv_on_gpu: true,
    }
}

fn parts(model: u64, context: u64, compute: u64, image_reader: u64) -> Parts {
    Parts {
        model,
        context,
        compute,
        image_reader,
    }
}

// ---- split ------------------------------------------------------------------------------

#[test]
fn split_a_dense_model() {
    let s = dense();
    let i = input(&s, vec![1, 2, 3, 4], 7, 3, 9);
    let all = split(&i, &on_card(Layers::All, ExpertsPick::None));
    assert_eq!(
        all,
        Split {
            gpu: parts(150, 10, 7, 9),
            cpu: parts(100, 0, 0, 0),
            layers_on_gpu: 5,
            units: 5,
            expert_layers_on_cpu: 0,
        }
    );
    // Three units: the output and blocks 3 and 2; blocks 0 and 1 on the processor.
    let three = split(&i, &on_card(Layers::Count(3), ExpertsPick::None));
    assert_eq!((three.gpu, three.cpu), (parts(120, 7, 7, 9), parts(130, 3, 3, 0)));
    assert_eq!(three.layers_on_gpu, 3);
    // One unit is the output alone.
    let one = split(&i, &on_card(Layers::Count(1), ExpertsPick::None));
    assert_eq!((one.gpu, one.cpu), (parts(50, 0, 7, 9), parts(200, 10, 3, 0)));
    // None: the processor holds the output and its working space; the image reader stays
    // with the graphics card.
    let none = split(&i, &on_card(Layers::Count(0), ExpertsPick::None));
    assert_eq!((none.gpu, none.cpu), (parts(0, 0, 0, 9), parts(250, 10, 7, 0)));
    assert_eq!(none.layers_on_gpu, 0);
    // More units than the model has is all of them.
    assert_eq!(split(&i, &on_card(Layers::Count(99), ExpertsPick::None)), all);
    assert_eq!(
        split(&i, &on_card(Layers::Count(u32::MAX), ExpertsPick::None)),
        all
    );
    // No graphics device: everything in computer memory, whatever else is asked.
    let cpu = split(
        &i,
        &Where {
            graphics: false,
            layers: Layers::All,
            experts_on_cpu: ExpertsPick::All,
            kv_on_gpu: true,
        },
    );
    assert_eq!((cpu.gpu, cpu.cpu), (Parts::default(), parts(250, 10, 7, 9)));
    assert_eq!((cpu.layers_on_gpu, cpu.units), (0, 5));
    // Experts asked of a dense model change nothing.
    assert_eq!(split(&i, &on_card(Layers::All, ExpertsPick::All)), all);
}

#[test]
fn split_with_conversation_memory_in_computer_memory() {
    let s = dense();
    let i = input(&s, vec![1, 2, 3, 4], 7, 3, 0);
    let place = |layers| Where {
        graphics: true,
        layers,
        experts_on_cpu: ExpertsPick::None,
        kv_on_gpu: false,
    };
    // The processor now does the attention, so it needs working space too.
    let all = split(&i, &place(Layers::All));
    assert_eq!((all.gpu, all.cpu), (parts(150, 0, 7, 0), parts(100, 10, 3, 0)));
    let three = split(&i, &place(Layers::Count(3)));
    assert_eq!((three.gpu, three.cpu), (parts(120, 0, 7, 0), parts(130, 10, 3, 0)));
    // Fewer per-block figures than blocks count the rest as 0; more are ignored.
    let short = input(&s, vec![1], 0, 0, 0);
    assert_eq!(
        split(&short, &on_card(Layers::All, ExpertsPick::None)).gpu.context,
        1
    );
    let long = input(&s, vec![1; 9], 0, 0, 0);
    assert_eq!(
        split(&long, &on_card(Layers::All, ExpertsPick::None)).gpu.context,
        4
    );
}

#[test]
fn other_tensors_go_with_the_output() {
    let s = sizes(100, 50, &[(10, 0), (20, 0)], 5);
    let i = input(&s, vec![], 0, 0, 0);
    assert_eq!(
        split(&i, &on_card(Layers::All, ExpertsPick::None)).gpu.model,
        85
    );
    assert_eq!(
        split(&i, &on_card(Layers::Count(0), ExpertsPick::None))
            .cpu
            .model,
        185
    );
}

#[test]
fn split_expert_parts() {
    let s = uneven_experts();
    let i = input(&s, vec![1, 2, 3, 4], 7, 3, 0);
    let case = |layers, experts| {
        let r = split(&i, &on_card(layers, experts));
        (r.gpu, r.cpu, r.expert_layers_on_cpu)
    };
    use ExpertsPick as E;
    use Layers as L;
    assert_eq!(
        case(L::All, E::None),
        (parts(1065, 10, 7, 0), parts(1000, 0, 0, 0), 0)
    );
    assert_eq!(
        case(L::All, E::All),
        (parts(65, 10, 7, 0), parts(2000, 0, 3, 0), 4)
    );
    assert_eq!(
        case(L::All, E::FirstLayers(2)),
        (parts(765, 10, 7, 0), parts(1300, 0, 3, 0), 2)
    );
    assert_eq!(
        case(L::All, E::LastLayers(1)),
        (parts(665, 10, 7, 0), parts(1400, 0, 3, 0), 1)
    );
    // Three units (blocks 2 and 3 on the card). The first K = 1 ≤ units − N = 2 blocks
    // are on the processor already: nothing more moves.
    assert_eq!(
        case(L::Count(3), E::FirstLayers(1)),
        (parts(762, 7, 7, 0), parts(1303, 3, 3, 0), 0)
    );
    assert_eq!(
        case(L::Count(3), E::FirstLayers(2)),
        case(L::Count(3), E::FirstLayers(1))
    );
    // K = 3 > units − N: block 2's expert parts move.
    assert_eq!(
        case(L::Count(3), E::FirstLayers(3)),
        (parts(462, 7, 7, 0), parts(1603, 3, 3, 0), 1)
    );
    assert_eq!(
        case(L::Count(3), E::LastLayers(1)),
        (parts(362, 7, 7, 0), parts(1703, 3, 3, 0), 1)
    );
    assert_eq!(
        case(L::Count(3), E::LastLayers(3)),
        (parts(62, 7, 7, 0), parts(2003, 3, 3, 0), 2)
    );
    assert_eq!(case(L::Count(3), E::LastLayers(3)), case(L::Count(3), E::All));
    assert_eq!(
        case(L::Count(3), E::LastLayers(0)),
        case(L::Count(3), E::None)
    );
    assert_eq!(
        case(L::Count(3), E::FirstLayers(0)),
        case(L::Count(3), E::None)
    );
    // Conversation memory in computer memory with expert parts moved.
    let r = split(
        &i,
        &Where {
            graphics: true,
            layers: L::Count(3),
            experts_on_cpu: E::All,
            kv_on_gpu: false,
        },
    );
    assert_eq!((r.gpu, r.cpu), (parts(62, 0, 7, 0), parts(2003, 10, 3, 0)));
    // Nothing on the card: no expert layer counts as moved.
    assert_eq!(
        case(L::Count(0), E::All),
        (Parts::default(), parts(2065, 10, 7, 0), 0)
    );
    // A block without expert parts is not counted.
    let first_dense = sizes(0, 0, &[(5, 0), (5, 50), (5, 50)], 0);
    let r = split(
        &input(&first_dense, vec![], 0, 0, 0),
        &on_card(L::All, E::FirstLayers(2)),
    );
    assert_eq!((r.expert_layers_on_cpu, r.cpu.model), (1, 50));
}

#[test]
fn effective_expert_layers_counts_only_blocks_on_the_card() {
    // 48 blocks; with 29 of 49 units on the card the first 20 blocks are on the processor.
    assert_eq!(effective_expert_layers(49, Layers::Count(29), 9), 0);
    assert_eq!(effective_expert_layers(49, Layers::Count(29), 20), 0);
    assert_eq!(effective_expert_layers(49, Layers::Count(29), 25), 5);
    assert_eq!(effective_expert_layers(49, Layers::Count(29), 1000), 28);
    assert_eq!(effective_expert_layers(49, Layers::All, 20), 20);
    assert_eq!(effective_expert_layers(49, Layers::Count(49), 20), 20);
    assert_eq!(effective_expert_layers(49, Layers::Count(100), 20), 20);
    assert_eq!(effective_expert_layers(49, Layers::All, 100), 48);
    assert_eq!(effective_expert_layers(49, Layers::Count(0), 20), 0);
    assert_eq!(effective_expert_layers(49, Layers::Count(1), 48), 0);
    assert_eq!(effective_expert_layers(0, Layers::All, 5), 0);
    assert_eq!(effective_expert_layers(1, Layers::All, 5), 0);
    assert_eq!(
        effective_expert_layers(u32::MAX, Layers::Count(u32::MAX), u32::MAX),
        u32::MAX - 1
    );
    // The same count split() reports, for every N and K, when every block has experts.
    let s = even_experts();
    let i = input(&s, vec![], 0, 0, 0);
    let units = s.units();
    for n in 0..=units + 1 {
        for k in 0..=units + 1 {
            let place = on_card(Layers::Count(n), ExpertsPick::FirstLayers(k));
            assert_eq!(
                split(&i, &place).expert_layers_on_cpu,
                effective_expert_layers(units, Layers::Count(n), k),
                "n {n} k {k}"
            );
        }
    }
}

#[test]
fn split_never_panics_on_huge_numbers() {
    let s = sizes(
        u64::MAX,
        u64::MAX,
        &[(u64::MAX, u64::MAX), (u64::MAX, u64::MAX)],
        u64::MAX,
    );
    let i = input(&s, vec![u64::MAX; 2], u64::MAX, u64::MAX, u64::MAX);
    for place in fit_order(&s) {
        let r = split(&i, &on_card(place.0, place.1));
        assert_eq!(r.gpu.total().max(r.cpu.total()), u64::MAX);
        let _ = judge(&r, Room::graphics_card(u64::MAX, u64::MAX), false);
        let _ = judge_in(&r, Room::default(), true, MemoryKind::Separate);
        let _ = bars(
            &r,
            MemoryKind::Separate,
            Some(Capacity {
                total_bytes: 1,
                free_bytes: u64::MAX,
            }),
            Capacity::default(),
        );
    }
    assert_eq!(fit_layers(&i, u64::MAX), (Layers::All, ExpertsPick::None));
    assert_eq!(fit_layers(&i, 0), (Layers::Count(0), ExpertsPick::None));
    let _ = fit_layers(&i, u64::MAX - 1);
    let _ = explain(&s, u64::MAX);
    let _ = explain(&s, 0);
}

// ---- fit_layers -------------------------------------------------------------------------

#[test]
fn fit_layers_for_a_dense_model() {
    let s = dense();
    // On the card: All 155 (output 50 + blocks 100 + working space 5), 4 units 145,
    // 3 units 125, 2 units 95, 1 unit 55, none 0.
    let i = input(&s, vec![0; 4], 5, 3, 0);
    let fit = |target| fit_layers(&i, target);
    assert_eq!(fit(155), (Layers::All, ExpertsPick::None), "fits whole");
    assert_eq!(fit(154), (Layers::Count(4), ExpertsPick::None));
    assert_eq!(fit(125), (Layers::Count(3), ExpertsPick::None));
    assert_eq!(fit(124), (Layers::Count(2), ExpertsPick::None));
    assert_eq!(fit(55), (Layers::Count(1), ExpertsPick::None));
    assert_eq!(fit(54), (Layers::Count(0), ExpertsPick::None), "less than one unit");
    assert_eq!(fit(0), (Layers::Count(0), ExpertsPick::None));
    // The image reader is not counted: llama.cpp's fit does not measure it.
    let with_reader = input(&s, vec![0; 4], 5, 3, 1000);
    assert_eq!(
        fit_layers(&with_reader, 155),
        (Layers::All, ExpertsPick::None)
    );
    // Conversation memory goes with the layers unless it is kept in computer memory.
    let with_kv = input(&s, vec![1; 4], 5, 3, 0);
    assert_eq!(fit_layers(&with_kv, 155), (Layers::Count(4), ExpertsPick::None));
    assert_eq!(
        fit_layers_where(&with_kv, 155, false),
        (Layers::All, ExpertsPick::None)
    );
}

#[test]
fn fit_layers_for_a_mixture_of_experts() {
    let s = even_experts();
    let i = input(&s, vec![0; 6], 0, 0, 0);
    let fit = |target| fit_layers(&i, target);
    use ExpertsPick as E;
    use Layers as L;
    // Everything: 20 + 6 × 80 = 500.
    assert_eq!(fit(500), (L::All, E::None), "fits whole");
    // Every dense part with every expert part on the processor: 20 + 180 = 200, so with
    // 150 llama.cpp stops after step 3: dense-only layers back to front (the frontmost
    // block on the card keeps its expert parts): 1 unit 20, 2 units 100, 3 units 130,
    // 4 units 160.
    assert_eq!(fit(150), (L::Count(3), E::LastLayers(1)));
    assert_eq!(fit(199), (L::Count(5), E::LastLayers(3)));
    assert_eq!(fit(101), (L::Count(2), E::None));
    // Step 4: all 7 units (250 with block 0 full), then full layers from the front:
    // the expert parts of the last K blocks stay on the processor (300, 350, … 500).
    assert_eq!(fit(360), (L::All, E::LastLayers(3)));
    assert_eq!(fit(300), (L::All, E::LastLayers(4)));
    assert_eq!(fit(499), (L::All, E::LastLayers(1)));
    // 230: step 3 ends at 6 units (220); step 4 cannot free a further block there.
    assert_eq!(fit(230), (L::Count(6), E::LastLayers(4)));
    assert_eq!(fit(19), (L::Count(0), E::None), "less than one unit");
    assert_eq!(fit(0), (L::Count(0), E::None));
}

#[test]
fn free_below_the_margin_gives_target_0() {
    assert_eq!(fit_target(10 * GB, GB), 9 * GB);
    assert_eq!(fit_target(GB / 2, GB), 0);
    assert_eq!(fit_target(0, u64::MAX), 0);
    let s = dense();
    let i = input(&s, vec![0; 4], 5, 3, 0);
    assert_eq!(
        fit_layers(&i, fit_target(3, 4)),
        (Layers::Count(0), ExpertsPick::None)
    );
}

// ---- explain ----------------------------------------------------------------------------

fn model_on_card(s: &LayerSizes, place: (Layers, ExpertsPick)) -> u64 {
    split(&input(s, vec![], 0, 0, 0), &on_card(place.0, place.1))
        .gpu
        .model
}

#[test]
fn fit_order_is_fits_own_order() {
    use ExpertsPick as E;
    use Layers as L;
    assert_eq!(
        fit_order(&dense()),
        vec![
            (L::Count(0), E::None),
            (L::Count(1), E::None),
            (L::Count(2), E::None),
            (L::Count(3), E::None),
            (L::Count(4), E::None),
            (L::All, E::None),
        ]
    );
    let order = fit_order(&even_experts());
    assert_eq!(
        order,
        vec![
            (L::Count(0), E::None),
            (L::Count(1), E::None),
            (L::Count(2), E::None),
            (L::Count(3), E::LastLayers(1)),
            (L::Count(4), E::LastLayers(2)),
            (L::Count(5), E::LastLayers(3)),
            (L::Count(6), E::LastLayers(4)),
            (L::All, E::LastLayers(5)),
            (L::All, E::LastLayers(4)),
            (L::All, E::LastLayers(3)),
            (L::All, E::LastLayers(2)),
            (L::All, E::LastLayers(1)),
            (L::All, E::None),
        ]
    );
    let bytes: Vec<u64> = order
        .iter()
        .map(|&p| model_on_card(&even_experts(), p))
        .collect();
    assert_eq!(
        bytes,
        vec![0, 20, 100, 130, 160, 190, 220, 250, 300, 350, 400, 450, 500]
    );
    // fit_layers only ever answers with a placement of this order.
    let s = even_experts();
    let i = input(&s, vec![0; 6], 0, 0, 0);
    for target in (0..=520).step_by(5) {
        let got = fit_layers(&i, target);
        assert!(
            order.contains(&got) || matches!(got, (L::Count(6), _)),
            "{target}: {got:?}"
        );
    }
}

#[test]
fn explain_round_trips_split() {
    for s in [dense(), even_experts(), uneven_experts()] {
        for place in fit_order(&s) {
            let bytes = model_on_card(&s, place);
            assert_eq!(explain(&s, bytes), Some(place), "{s:?} {place:?}");
        }
    }
}

#[test]
fn explain_gives_none_between_steps() {
    use ExpertsPick as E;
    use Layers as L;
    // Dense: 0, 50, 90, 120, 140, 150.
    let s = dense();
    assert_eq!(explain(&s, 52), Some((L::Count(1), E::None)));
    assert_eq!(explain(&s, 70), None);
    assert_eq!(explain(&s, 105), None);
    assert_eq!(explain(&s, 151), Some((L::All, E::None)));
    assert_eq!(explain(&s, 1_000), None);
    // Experts: … 100, 130 … 450, 500.
    let s = even_experts();
    assert_eq!(explain(&s, 102), Some((L::Count(2), E::None)));
    assert_eq!(explain(&s, 115), None);
    assert_eq!(explain(&s, 325), None);
    assert_eq!(explain(&s, 505), Some((L::All, E::None)));
    assert_eq!(explain(&s, 600), None);
    // Nothing weighs anything: only an exact 0 is explained.
    let empty = sizes(0, 0, &[(0, 0), (0, 0)], 0);
    assert_eq!(explain(&empty, 0), Some((L::Count(0), E::None)));
    assert_eq!(explain(&empty, 1), None);
}

// ---- judge ------------------------------------------------------------------------------

/// A split with `gpu` and `cpu` bytes of model.
fn sp(gpu: u64, cpu: u64, layers_on_gpu: u32, units: u32, experts: u32) -> Split {
    Split {
        gpu: parts(gpu, 0, 0, 0),
        cpu: parts(cpu, 0, 0, 0),
        layers_on_gpu,
        units,
        expert_layers_on_cpu: experts,
    }
}

#[test]
fn judge_gives_each_answer() {
    use Fits as F;
    use Verdict as V;
    // A 10 GB card and 20 GB of computer memory: 28 GB together after the reserve.
    let card = Room::graphics_card(10 * GB, 20 * GB);
    assert_eq!((card.whole, card.partial), (10 * GB, 28 * GB));
    let j = |s: Split| judge(&s, card, false);
    assert_eq!(j(sp(8 * GB, GB, 5, 5, 0)), (V::Green, F::Fits));
    assert_eq!(j(sp(9 * GB + 1, GB, 5, 5, 0)), (V::Yellow, F::Tight));
    assert_eq!(j(sp(10 * GB, GB, 5, 5, 0)), (V::Yellow, F::Tight));
    assert_eq!(j(sp(10 * GB + 1, GB, 5, 5, 0)), (V::Yellow, F::WontFitGraphics));
    assert_eq!(j(sp(12 * GB, 15 * GB, 5, 5, 0)), (V::Yellow, F::WontFitGraphics));
    assert_eq!(j(sp(12 * GB, 15 * GB + 1, 5, 5, 0)), (V::Red, F::WontFitAtAll));
    // As much as fits: part on the processor is expected, never wontFitGraphics.
    let f = |s: Split| judge(&s, card, true);
    assert_eq!(f(sp(8 * GB, GB, 5, 5, 0)), (V::Green, F::Fits));
    assert_eq!(f(sp(8 * GB, GB, 3, 5, 0)), (V::Yellow, F::Partial));
    assert_eq!(f(sp(8 * GB, GB, 5, 5, 2)), (V::Yellow, F::Partial));
    assert_eq!(f(sp(9 * GB + 1, GB, 5, 5, 0)), (V::Yellow, F::Tight));
    assert_eq!(f(sp(11 * GB, GB, 3, 5, 0)), (V::Yellow, F::Partial));
    assert_eq!(f(sp(12 * GB, 30 * GB, 3, 5, 0)), (V::Red, F::WontFitAtAll));
}

/// On the processor only, judge is fit.rs's Room::verdict of the total.
#[test]
fn judge_on_the_processor_is_room_verdict() {
    for room in [
        Room::processor(12 * GB),
        Room::processor(GB),
        Room::processor(0),
    ] {
        for total in [0, 5 * GB, 9 * GB, 9 * GB + 1, 10 * GB, 40 * GB, u64::MAX] {
            let expected = room.verdict(total);
            let fits = match expected {
                Verdict::Green => Fits::Fits,
                Verdict::Yellow => Fits::Partial,
                Verdict::Red => Fits::WontFitAtAll,
            };
            for fit in [false, true] {
                let s = Split {
                    gpu: Parts::default(),
                    cpu: parts(total / 2, total / 4, total - total / 2 - total / 4, 0),
                    layers_on_gpu: 0,
                    units: 5,
                    expert_layers_on_cpu: 0,
                };
                assert_eq!(judge(&s, room, fit), (expected, fits), "{total}");
                assert_eq!(judge_in(&s, room, fit, MemoryKind::Processor), (expected, fits));
            }
        }
    }
}

/// With everything on the graphics side, judge's mark is Room::verdict on every room.
#[test]
fn judge_on_one_device_is_room_verdict() {
    for room in [
        Room::graphics_card(10 * GB, 20 * GB),
        Room::graphics_card(4 * GB, GB),
        Room::apple(11 * GB, 9 * GB),
        Room::apple(11 * GB, 15 * GB),
    ] {
        for need in [0, 5 * GB, 9 * GB, 10 * GB, 11 * GB, 13 * GB, 30 * GB] {
            let s = sp(need, 0, 5, 5, 0);
            assert_eq!(judge(&s, room, false).0, room.verdict(need), "{room:?} {need}");
        }
    }
}

/// A discrete card keeps the processor's part in computer memory on its own; Apple
/// silicon shares one memory.
#[test]
fn memory_kinds() {
    use Fits as F;
    use Verdict as V;
    let card = Room::graphics_card(10 * GB, 20 * GB);
    // 2 GB on the card, 20 GB on the processor: together it is under 28 GB, but the
    // computer has only 18 GB after the reserve.
    let s = sp(2 * GB, 20 * GB, 1, 5, 0);
    assert_eq!(judge(&s, card, false), (V::Green, F::Fits));
    assert_eq!(
        judge_in(&s, card, false, MemoryKind::Separate),
        (V::Red, F::WontFitAtAll)
    );
    assert_eq!(
        judge_in(&sp(2 * GB, 18 * GB, 1, 5, 0), card, false, MemoryKind::Separate),
        (V::Green, F::Fits)
    );
    // A 16 GB Mac: 11 GB working set, 9 GB available (7 GB after the reserve).
    let mac = Room::apple(11 * GB, 9 * GB);
    let m = sp(9 * GB, GB / 2, 5, 5, 0);
    assert_eq!(judge(&m, mac, false), (V::Green, F::Fits));
    assert_eq!(judge_in(&m, mac, false, MemoryKind::Unified), (V::Green, F::Fits));
    assert_eq!(
        judge(&sp(10 * GB, GB / 2, 5, 5, 0), mac, false),
        (V::Red, F::WontFitAtAll)
    );
    // Nothing on the card of a discrete room: the computer part is what counts.
    let none = sp(0, 17 * GB, 0, 5, 0);
    assert_eq!(
        judge_in(&none, card, false, MemoryKind::Separate),
        (V::Green, F::Fits)
    );
}

// ---- auto_ctx_placed --------------------------------------------------------------------

/// An 8B-class model of 4.9 GB in 32 blocks; f16 conversation memory 131,072 bytes per
/// token (4,096 per block).
fn eight_b(input_bytes: u64) -> LayerSizes {
    let output = 900_000_000 - input_bytes;
    sizes(input_bytes, output, &[(125_000_000, 0); 32], 0)
}

fn eight_b_fit(max_ctx: Option<u64>, projector: u64) -> FitInput {
    FitInput {
        weights_bytes: 4_900_000_000,
        projector_bytes: projector,
        kv_bytes_per_token: 131_072,
        vocab_size: 128_256,
        max_ctx,
        ..Default::default()
    }
}

fn floor_cap(max_ctx: Option<u64>) -> (u64, u64) {
    (
        max_ctx.map_or(fit::AUTO_FLOOR_CTX, |m| m.min(fit::AUTO_FLOOR_CTX)),
        max_ctx.map_or(fit::AUTO_CAP_CTX, |m| m.min(fit::AUTO_CAP_CTX)),
    )
}

#[test]
fn auto_ctx_placed_matches_auto_ctx_on_one_device() {
    let trained = [Some(131_072), Some(2048), Some(20_000), Some(5_000), None];
    // The processor: everything in computer memory.
    let s = eight_b(500_000_000);
    assert_eq!(s.total(), 4_900_000_000);
    for room in [
        Room::processor(20 * GB),
        Room::processor(12 * GB),
        Room::processor(8 * GB),
        Room::processor(7 * GB),
        Room::processor(GB),
    ] {
        for max_ctx in trained {
            for projector in [0, 600_000_000] {
                let fit_input = eight_b_fit(max_ctx, projector);
                let at = |ctx: u64| {
                    let i = input(
                        &s,
                        vec![4096 * ctx; 32],
                        fit_input.compute_bytes(),
                        COMPUTE_BASE_BYTES,
                        projector,
                    );
                    split(
                        &i,
                        &Where {
                            graphics: false,
                            layers: Layers::All,
                            experts_on_cpu: ExpertsPick::None,
                            kv_on_gpu: true,
                        },
                    )
                };
                let (floor, cap) = floor_cap(max_ctx);
                assert_eq!(
                    auto_ctx_placed(at, room, floor, cap),
                    fit::auto_ctx(&fit_input, room),
                    "{room:?} {max_ctx:?} {projector}"
                );
            }
        }
    }
    // A graphics device holding everything (no input layer in this model).
    let s = eight_b(0);
    for room in [
        Room::graphics_card(15 * GB, 24 * GB),
        Room::graphics_card(7_500_000_000, 24 * GB),
        Room::graphics_card(4 * GB, 24 * GB),
        Room::graphics_card(4 * GB, GB),
        Room::apple(11 * GB, 9 * GB),
        Room::apple(7 * GB, 30 * GB),
    ] {
        for max_ctx in trained {
            for projector in [0, 600_000_000] {
                let fit_input = eight_b_fit(max_ctx, projector);
                let at = |ctx: u64| {
                    let i = input(
                        &s,
                        vec![4096 * ctx; 32],
                        fit_input.compute_bytes(),
                        COMPUTE_BASE_BYTES,
                        projector,
                    );
                    split(&i, &on_card(Layers::All, ExpertsPick::None))
                };
                let (floor, cap) = floor_cap(max_ctx);
                let expected = fit::auto_ctx(&fit_input, room);
                assert_eq!(
                    auto_ctx_placed(&at, room, floor, cap),
                    expected,
                    "{room:?} {max_ctx:?} {projector}"
                );
                assert_eq!(
                    auto_ctx_placed_in(&at, room, floor, cap, MemoryKind::Separate),
                    expected
                );
            }
        }
    }
}

/// A custom placement: the largest size whose split is Green, the next size up is not.
#[test]
fn auto_ctx_placed_for_a_custom_placement() {
    let s = eight_b(500_000_000);
    let fit_input = eight_b_fit(Some(131_072), 0);
    let at = |ctx: u64| {
        let i = input(
            &s,
            vec![4096 * ctx; 32],
            fit_input.compute_bytes(),
            COMPUTE_BASE_BYTES,
            0,
        );
        split(&i, &on_card(Layers::Count(17), ExpertsPick::None))
    };
    let room = Room::graphics_card(5 * GB, 16 * GB);
    let got = auto_ctx_placed(at, room, 4096, 32_768).unwrap();
    assert_eq!(got.verdict, Verdict::Green);
    assert!(got.ctx < 32_768 && got.ctx.is_multiple_of(1024), "{got:?}");
    assert_eq!(judge(&at(got.ctx), room, false).0, Verdict::Green);
    assert_ne!(judge(&at(got.ctx + 1024), room, false).0, Verdict::Green);
    // Too little for even the floor on the card, but the computer holds it: Yellow.
    let small = Room::graphics_card(2 * GB, 16 * GB);
    let got = auto_ctx_placed(at, small, 4096, 32_768).unwrap();
    assert_eq!(got.verdict, Verdict::Yellow);
    // Not even the floor at all.
    assert_eq!(auto_ctx_placed(at, Room::graphics_card(GB, GB), 4096, 32_768), None);
}

#[test]
fn auto_ctx_placed_never_panics_or_hangs() {
    let huge = |_: u64| Split {
        gpu: parts(u64::MAX, u64::MAX, u64::MAX, u64::MAX),
        cpu: parts(u64::MAX, 0, 0, 0),
        layers_on_gpu: u32::MAX,
        units: u32::MAX,
        expert_layers_on_cpu: u32::MAX,
    };
    let any = Room {
        whole: u64::MAX,
        partial: u64::MAX,
    };
    let big = Room {
        whole: 1000 * GB,
        partial: 1000 * GB,
    };
    assert_eq!(auto_ctx_placed(huge, big, 0, u64::MAX), None);
    let nothing = |_: u64| sp(0, 0, 5, 5, 0);
    assert_eq!(
        auto_ctx_placed(nothing, any, 0, u64::MAX),
        Some(AutoCtx {
            ctx: u64::MAX,
            verdict: Verdict::Green
        })
    );
    assert_eq!(
        auto_ctx_placed(nothing, any, u64::MAX, 0),
        Some(AutoCtx {
            ctx: 0,
            verdict: Verdict::Green
        })
    );
    assert_eq!(
        auto_ctx_placed(nothing, any, 3000, 3000).map(|a| a.ctx),
        Some(3000)
    );
    // The candidates are the floor, the multiples of 1,024 between, and the cap.
    let up_to = |limit: u64| move |ctx: u64| sp(if ctx <= limit { 0 } else { u64::MAX }, 0, 5, 5, 0);
    assert_eq!(
        auto_ctx_placed(up_to(20_000), big, 4096, 32_768).map(|a| a.ctx),
        Some(19_456)
    );
    assert_eq!(
        auto_ctx_placed(up_to(20_000), big, 4096, 20_000).map(|a| a.ctx),
        Some(20_000)
    );
    assert_eq!(
        auto_ctx_placed(up_to(5000), big, 4500, 32_768).map(|a| a.ctx),
        Some(4500)
    );
    assert_eq!(auto_ctx_placed(up_to(4000), big, 4096, 32_768), None);
}

// ---- conversation memory per block ------------------------------------------------------

#[test]
fn kv_per_layer_follows_the_header() {
    use crate::gguf::testing::Writer;
    let read = |bytes: Vec<u8>| {
        let len = bytes.len() as u64;
        ModelSummary::from_header(&crate::gguf::read(std::io::Cursor::new(bytes), len).unwrap())
    };
    let llama = read(
        Writer::new()
            .str("general.architecture", "llama")
            .u32("llama.block_count", 32)
            .u32("llama.embedding_length", 4096)
            .u32("llama.attention.head_count", 32)
            .u32("llama.attention.head_count_kv", 8)
            .build(),
    );
    for cache in [KvCacheType::F16, KvCacheType::Q8_0] {
        let per = kv_per_layer(&llama, 1000, cache);
        assert_eq!(per.len(), 32);
        assert_eq!(
            per.iter().sum::<u64>(),
            llama.kv_bytes_per_token_as(cache) * 1000
        );
    }
    assert_eq!(kv_per_layer(&llama, 1, KvCacheType::F16)[0], 4096);
    let hybrid = read(
        Writer::new()
            .str("general.architecture", "lfm2")
            .u32("lfm2.block_count", 4)
            .u32("lfm2.embedding_length", 1024)
            .u32("lfm2.attention.head_count", 16)
            .u32_array("lfm2.attention.head_count_kv", &[8, 0, 8, 0])
            .build(),
    );
    assert_eq!(
        kv_per_layer(&hybrid, 10, KvCacheType::F16),
        vec![20_480, 0, 20_480, 0]
    );
    assert_eq!(
        kv_per_layer(&llama, u64::MAX, KvCacheType::F16)[0],
        u64::MAX
    );
}

// ---- bars -------------------------------------------------------------------------------

#[test]
fn bars_add_up() {
    // The worked example of the picture, in units of 0.1 GB: a 15.9 card with 15.0 free
    // and a 31.9 computer with 19.9 free.
    let graphics = Capacity {
        total_bytes: 159,
        free_bytes: 150,
    };
    let computer = Capacity {
        total_bytes: 319,
        free_bytes: 199,
    };
    let s = Split {
        gpu: parts(99, 12, 6, 0),
        cpu: parts(41, 4, 3, 0),
        layers_on_gpu: 30,
        units: 41,
        expert_layers_on_cpu: 0,
    };
    let (g, c) = bars(&s, MemoryKind::Separate, Some(graphics), computer);
    let g = g.unwrap();
    assert_eq!(
        (g.total_bytes, g.free_bytes, g.other_bytes, g.need, g.free_after_bytes),
        (159, 150, 9, s.gpu, 33)
    );
    assert_eq!(g.need.total() + g.other_bytes + g.free_after_bytes, 159);
    assert_eq!(
        (c.total_bytes, c.free_bytes, c.other_bytes, c.need, c.free_after_bytes),
        (319, 199, 120, s.cpu, 151)
    );
    // Apple silicon: one bar with both parts.
    let (g, c) = bars(&s, MemoryKind::Unified, Some(graphics), computer);
    assert_eq!(g, None);
    assert_eq!(c.need, parts(140, 16, 9, 0));
    assert_eq!(c.free_after_bytes, 199 - 165);
    let (g, c) = bars(&s, MemoryKind::Processor, None, computer);
    assert_eq!((g, c.need), (None, parts(140, 16, 9, 0)));
    // More than is free: nothing free after loading, never an underflow.
    let full = bar(graphics, parts(200, 0, 0, 0));
    assert_eq!((full.free_after_bytes, full.other_bytes), (0, 9));
    // A free figure above the total is cut to the total; an unknown total is the free.
    let odd = bar(
        Capacity {
            total_bytes: 10,
            free_bytes: 12,
        },
        Parts::default(),
    );
    assert_eq!((odd.total_bytes, odd.free_bytes, odd.other_bytes), (10, 10, 0));
    let unknown = bar(
        Capacity {
            total_bytes: 0,
            free_bytes: 12,
        },
        Parts::default(),
    );
    assert_eq!((unknown.total_bytes, unknown.free_bytes), (12, 12));
}

#[test]
fn serialized_names_are_camel_case() {
    let s = sp(1, 2, 3, 4, 5);
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["layersOnGpu"], 3);
    assert_eq!(json["expertLayersOnCpu"], 5);
    assert_eq!(json["gpu"]["imageReader"], 0);
    assert_eq!(
        serde_json::to_value(Fits::WontFitGraphics).unwrap(),
        "wontFitGraphics"
    );
    assert_eq!(
        serde_json::to_value(Fits::WontFitAtAll).unwrap(),
        "wontFitAtAll"
    );
}
