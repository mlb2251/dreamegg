use crate::*;
use ahash::{AHashMap, AHashSet};
// use std::collections::BTreeSet;
use std::fmt::{self, Formatter, Display};
use std::hash::Hash;
use itertools::Itertools;
use extraction::extract;
use serde_json::json;
use clap::{Parser};
use serde::Serialize;
use std::thread;
use std::sync::Arc;
use parking_lot::Mutex;
use std::ops::DerefMut;
use std::collections::BinaryHeap;
use rand::Rng;
use std::iter::once;

/// Args for compression step
#[derive(Parser, Debug, Serialize, Clone)]
#[clap(name = "Stitch")]
pub struct CompressionStepConfig {
    /// max arity of inventions to find (will find all from 0 to this number inclusive)
    #[clap(short='a', long, default_value = "2")]
    pub max_arity: usize,

    /// num threads (no parallelism if set to 1)
    #[clap(short='t', long, default_value = "1")]
    pub threads: usize,

    /// how many worklist items a thread will take at once
    #[clap(short='b', long, default_value = "1")]
    pub batch: usize,

    /// threads will autoadjust how large their batches are based on the worklist size
    #[clap(long)]
    pub dynamic_batch: bool,

    /// disables refinement
    #[clap(long)]
    pub refine: bool,

    /// max refinement size
    #[clap(long)]
    pub max_refinement_size: Option<i32>,

    /// max number of refined out args that can be passed into a #i
    #[clap(long, default_value = "1")]
    pub max_refinement_arity: usize,

    /// Number of invention candidates compression_step should return. Raising this may weaken the efficacy of upper bound pruning
    #[clap(short='n', long, default_value = "1")]
    pub inv_candidates: usize,

    /// strategy for picking the next hole to expand
    #[clap(long, arg_enum, default_value = "depth-first")]
    pub hole_choice: HoleChoice,

    /// strategy for picking the next worklist item to process
    #[clap(long, arg_enum, default_value = "max-bound")]
    pub heap_choice: HeapChoice,

    /// disables the safety check for the utility being correct; you only want
    /// to do this if you truly dont mind unsoundness for a minute
    #[clap(long)]
    pub no_mismatch_check: bool,

    /// inventions cant start with a Lambda
    #[clap(long)]
    pub no_top_lambda: bool,

    /// pattern or invention to track
    #[clap(long)]
    pub track: Option<String>,

    /// refined version of pattern or invention to track
    #[clap(long)]
    pub track_refined: Option<String>,

    /// pattern or invention to track
    #[clap(long)]
    pub follow_track: bool,

    /// print out each step of what gets popped off the worklist
    #[clap(long)]
    pub verbose_worklist: bool,

    /// whenever a new best thing is found, print it
    #[clap(long)]
    pub verbose_best: bool,

    /// print stats this often (0 means never)
    #[clap(long, default_value = "0")]
    pub print_stats: usize,

    /// for dreamcoder comparison only: this makes stitch drop its final searchh
    /// result and return one less invention than you asked for while still
    /// doing the work of finding that last invention. This simulations how dreamcoder
    /// finds and rejects its final candidate
    #[clap(long)]
    pub dreamcoder_drop_last: bool,

    /// disable caching (though caching isn't used for much currently)
    #[clap(long)]
    pub no_cache: bool,

    /// print out programs rewritten under invention
    #[clap(long,short='r')]
    pub show_rewritten: bool,

    /// disable the free variable pruning optimization
    #[clap(long)]
    pub no_opt_free_vars: bool,

    /// disable the single structurally hashed subtree match pruning
    #[clap(long)]
    pub no_opt_single_use: bool,

    /// disable the single task pruning optimization
    #[clap(long)]
    pub no_opt_single_task: bool,

    /// disable the upper bound pruning optimization
    #[clap(long)]
    pub no_opt_upper_bound: bool,

    /// disable the force multiuse pruning optimization
    #[clap(long)]
    pub no_opt_force_multiuse: bool,

    /// disable the useless abstraction pruning optimization
    #[clap(long)]
    pub no_opt_useless_abstract: bool,

    /// disable the arity zero priming optimization
    #[clap(long)]
    pub no_opt_arity_zero: bool,

    /// Disable stat logging - note that stat logging in multithreading requires taking a mutex
    /// so it could be a source of slowdown in the multithreaded case, hence this flag to disable it.
    /// From some initial tests it seems to cause no slowdown anyways though.
    #[clap(long)]
    pub no_stats: bool,

    /// disables other_utility so the only utility is based on compressivity
    #[clap(long)]
    pub no_other_util: bool,

    /// whenever you finish an invention do a full rewrite to check that rewriting doesnt raise a mismatch exception
    #[clap(long)]
    pub rewrite_check: bool,

    /// anything related to running a dreamcoder comparison
    #[clap(long)]
    pub dreamcoder_comparison: bool,
    
}

impl CompressionStepConfig {
    pub fn no_opt(&mut self) {
        self.no_opt_free_vars = true;
        self.no_opt_single_task = true;
        self.no_opt_upper_bound = true;
        self.no_opt_force_multiuse = true;
        self.no_opt_useless_abstract = true;
        self.no_opt_arity_zero = true;
    }
}



/// A Pattern is a partial invention with holes. The simplest pattern is the single hole `??` which
/// matches at all nodes in the program set. From this single hole in a top-down manner we grow more complex
/// patterns like `(+ ?? ??)` and `(+ 3 (* ?? ??))`. Expanding a hole in a pattern always results in a pattern
/// that matches at a subset of the places that the original pattern matched.
/// 
/// `match_locations` is the list of structurally hashed nodes where the pattern matches.
/// `holes` is the list of zippers that point from the root of the pattern to the holes.
/// `arg_choices` is the same as `holes` but for the invention arguments like #i
/// `body_utility` is the cost of the non-hole non-argchoice parts of the pattern so far
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pattern {
    // pub holes: Vec<ZId>, // in order of when theyre added NOT left to right
    // arg_choices: Vec<LabelledZId>, // a hole gets moved into here when it becomes an argchoice, again these are in order of when they were added
    // pub first_zid_of_ivar: Vec<ZId>, //first_zid_of_ivar[i] gives the zid of the first use of #i in arg_choices
    pub refinements: Vec<Option<Vec<Id>>>, // refinements[i] gives the list of refinements for #i
    // pub match_locations: Option<MatchLocations>, // places where it applies
    pub utility_upper_bound: i32,
    pub body_utility_no_refinement: i32, // the size (in `cost`) of a single use of the pattern body so far
    pub refinement_body_utility: i32, // modifier on body_utility to include the full size account for refinement
    pub tracked: bool, // for debugging

    pub hole_zips: Vec<Zip>,
    // pub hole_unshifted_ids: Vec<Vec<Id>>, // hole_unshifted_ids[hole_idx][match_loc_idx]
    pub arg_zips: Vec<LabelledZip>,
    pub arity: usize,
    pub any_loc_id: Id, // just a random loc id that this pattern matches at, used to help with rendering it
    // pub arg_shifted_ids: Vec<Vec<Id>>,  // arg_shifted_ids[ivar][match_loc_idx]

}

type MatchLocations = Vec<MatchLocation>;

// #[derive(Debug, Clone, PartialEq, Eq, Hash)]
// pub struct ArgInfo {
//     pub unshifted_id: Id,
//     pub shift: i32,
// }


#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchLocation {
    pub id: Id, // unshifted id of subtree we're matching at
    pub hole_unshifted_ids: Vec<Id>,
    pub arg_shifted_ids: Vec<Shifted>,
    pub cached_expands_to: ExpandsTo,
    pub undo_hole_id: Vec<Id>,
    // pub arg_info: Vec<ArgInfo>,
}
impl MatchLocation {
    pub fn new(id: Id) -> MatchLocation {
        MatchLocation {
            id,
            hole_unshifted_ids: vec![id], // assumes we initially match against a singleton hole
            arg_shifted_ids: vec![],
            cached_expands_to: ExpandsTo::IVar(-626),
            undo_hole_id: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Shifted {
    pub downshifted_id: Id,
    pub shift: i32,
}
impl Shifted {
    pub fn new(downshifted_id: Id, shift: i32) -> Shifted {
        Shifted {
            downshifted_id,
            shift,
        }
    }
    #[inline]
    pub fn shifted_by(&self, shift: i32) -> Shifted {
        Shifted {
            downshifted_id: self.downshifted_id,
            shift: self.shift + shift,
        }
    }
    pub fn extract(&self, egraph: &crate::EGraph) -> Expr {
        extract_shifted(self.downshifted_id, self.shift, 0, egraph)
    }
}

impl Expr {
    fn zipper_replace(&self, zip: &Zip, new: &str) -> Expr {
        let child = self.apply_zipper(zip).unwrap();
        // clone and overwrite that node
        let mut res = self.clone();
        res.nodes[usize::from(child)] = Lambda::Prim(new.into());
        res
    }
    /// replaces the node at the end of the zipper with `new` prim,
    /// returning the new expression
    fn apply_zipper(&self, zip: &Zip) -> Option<Id> {
        let mut child = self.root();
        for znode in zip.iter() {
            child = match (znode, self.get(child)) {
                (ZNode::Body, Lambda::Lam([b])) => *b,
                (ZNode::Func, Lambda::App([f,_])) => *f,
                (ZNode::Arg, Lambda::App([_,x])) => *x,
                (_,_) => return None // no zipper works here
            };
        }
        Some(child)
    }
}

/// returns the vec of zippers to each ivar
fn zips_of_ivar_of_expr(expr: &Expr) -> Vec<Vec<Zip>> {

    // quickly determine arity
    let mut arity = 0;
    for node in expr.nodes.iter() {
        if let Lambda::IVar(ivar) = node {
            if ivar + 1 > arity {
                arity = ivar + 1;
            }
        }
    }

    let mut curr_zip: Zip = vec![];
    let mut zips_of_ivar = vec![vec![]; arity as usize];

    fn helper(curr_node: Id, expr: &Expr, curr_zip: &mut Zip, zips_of_ivar: &mut Vec<Vec<Zip>>) {
        match expr.get(curr_node) {
            Lambda::Prim(_) => {},
            Lambda::Var(_) => {},
            Lambda::IVar(i) => {
                zips_of_ivar[*i as usize].push(curr_zip.clone());
            },
            Lambda::Lam([b]) => {
                curr_zip.push(ZNode::Body);
                helper(*b, expr, curr_zip, zips_of_ivar);
                curr_zip.pop();
            }
            Lambda::App([f,x]) => {
                curr_zip.push(ZNode::Func);
                helper(*f, expr, curr_zip, zips_of_ivar);
                curr_zip.pop();
                curr_zip.push(ZNode::Arg);
                helper(*x, expr, curr_zip, zips_of_ivar);
                curr_zip.pop();
            }
            _ => unreachable!(),
        }
        
    }
    // we can pick any match location
    helper(expr.root(), expr, &mut curr_zip, &mut zips_of_ivar);

    zips_of_ivar
}


impl Pattern {
    // fn shatter(self) -> (Vec<Option<Vec<Id>>>,Vec<MatchLocation>,i32,i32,i32, bool, Vec<Zip>, Vec<LabelledZip>, usize)
    // {
    //     (self.refinements, self.match_locations, self.utility_upper_bound, self.body_utility_no_refinement, self.refinement_body_utility, self.tracked, self.hole_zips, self.arg_zips, self.arity)
    // }

    /// create a single hole pattern `??`
    #[inline(never)]
    fn single_hole(shared: &SharedData, match_locations: &[MatchLocation]) -> Self {
        // let mut match_locations: Vec<MatchLocation> = shared.treenodes.iter().map(|&id| MatchLocation::new(id, vec![id], vec![])).collect();
        // match_locations.sort_by_key(|m|m.id); // we assume match_locations is always sorted
        // if shared.cfg.no_top_lambda {
        //     match_locations.retain(|m| expands_to_of_node(&shared.node_of_id[usize::from(m.id)]) != ExpandsTo::Lam);
        // }
        let utility_upper_bound = utility_upper_bound(&match_locations, 0, &shared);
        Pattern {
            // holes: vec![EMPTY_ZID], // (zid 0 is the empty zipper)
            // arg_choices: vec![],
            // first_zid_of_ivar: vec![],
            refinements: vec![],
            // match_locations: Some(match_locations.clone()), // single hole matches everywhere
            utility_upper_bound,
            body_utility_no_refinement: 0, // 0 body utility
            refinement_body_utility: 0, // 0 body utility
            tracked: shared.cfg.track.is_some(),
            hole_zips: vec![vec![]], // there is one empty zipper
            // hole_unshifted_ids: vec![match_locations.clone()],
            arg_zips: vec![],
            arity: 0,
            any_loc_id: match_locations.first().map(|m| m.id).unwrap(),
            // arg_shifted_ids: vec![],
        }
    }
    /// convert pattern to an Expr with `??` in place of holes and `?#` in place of argchoices
    /// note `any_loc_id` can be the id of any match location, for example the first one
    fn to_expr(&self, shared: &SharedData) -> Expr {
        let mut curr_zip: Zip = vec![];
        // map zids to zips with a bool thats true if this is a hole and false if its a future ivar
        let zips: Vec<(&Zip,Expr)> = self.hole_zips.iter().map(|zip| (zip, Expr::prim("??".into())))
            .chain(self.arg_zips.iter().map(|labelled_zip| (&labelled_zip.zip,
                if let Some(refinements) = self.refinements[labelled_zip.ivar].as_ref() {

                    // extract the refinement and remap #i to $(i+depth) where depth is depth of #i in `extracted`
                    let mut extracted = refinements.iter().map(|refinement| extract(*refinement, &shared.egraph)).collect::<Vec<_>>();
                    extracted.iter_mut().for_each(|e| arg_ivars_to_vars(e));
                    let mut expr = Expr::ivar(labelled_zip.ivar as i32);
                    // todo are these applied in the right order?
                    for e in extracted {
                        expr = Expr::app(expr,e);
                    }
                    expr
                } else {
                    Expr::ivar(labelled_zip.ivar as i32)
                }))).collect();


        fn helper(curr_node: Id, curr_zip: &mut Zip, zips: &Vec<(&Zip,Expr)>, shared: &SharedData) -> Expr {
            match zips.iter().find(|(zip,_)| *zip == curr_zip) {
                // current zip matches a hole
                Some((_,e)) => e.clone(),
                // no ivar zip match, so recurse
                None => {
                    match &shared.node_of_id[usize::from(curr_node)] {
                        Lambda::Prim(p) => Expr::prim(*p),
                        Lambda::Var(v) => Expr::var(*v),
                        Lambda::Lam([b]) => {
                            curr_zip.push(ZNode::Body);
                            let b_expr = helper(*b, curr_zip, &zips, shared);
                            curr_zip.pop();
                            Expr::lam(b_expr) 
                        }
                        Lambda::App([f,x]) => {
                            curr_zip.push(ZNode::Func);
                            let f_expr = helper(*f, curr_zip, &zips, shared);
                            curr_zip.pop();
                            curr_zip.push(ZNode::Arg);
                            let x_expr = helper(*x, curr_zip, &zips, shared);
                            curr_zip.pop();
                            Expr::app(f_expr, x_expr)
                        }
                        _ => unreachable!(),
                    }
                }
            }
            
        }
        // we can pick any match location
        helper(self.any_loc_id, &mut curr_zip, &zips, shared)
    }
    fn show_track_expansion(&self, hole_zip: &Zip, shared: &SharedData) -> String {
        let mut s = self.to_expr(shared).zipper_replace(&hole_zip, &"<REPLACE>" ).to_string();
        s = s.replace(&"<REPLACE>", &format!("{}",tracked_expands_to(self, hole_zip, shared)).clone().magenta().bold().to_string());
        s
    }
    pub fn info(&self, shared: &SharedData, match_locations: &[MatchLocation]) -> String {
        format!("{}: utility_upper_bound={}, body_utility=({},{}), refinements={}, match_locations={}, usages={}",self.to_expr(shared), self.utility_upper_bound, self.body_utility_no_refinement, self.refinement_body_utility, self.refinements.iter().filter(|x|x.is_some()).count(), match_locations.len(), match_locations.iter().map(|loc|shared.num_paths_to_node[usize::from(loc.id)]).sum::<i32>())
    }
}

/// The child-ignoring value of a node in the original set of programs. This tells us
/// what the hole will expand into at this node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum ExpandsTo {
    Lam,
    App,
    Var(i32),
    Prim(Symbol),
    IVar(i32),
}

impl ExpandsTo {
    #[inline]
    /// true if expanding a node of this ExpandsTo will yield new holes
    fn has_holes(&self) -> bool {
        match self {
            ExpandsTo::Lam => true,
            ExpandsTo::App => true,
            ExpandsTo::Var(_) => false,
            ExpandsTo::Prim(_) => false,
            ExpandsTo::IVar(_) => false,
        }
    }
    #[inline]
    fn is_ivar(&self) -> bool {
        match self {
            ExpandsTo::IVar(_) => true,
            _ => false
        }
    }
}

impl std::fmt::Display for ExpandsTo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ExpandsTo::Lam => write!(f, "(lam ??)"),
            ExpandsTo::App => write!(f, "(?? ??)"),
            ExpandsTo::Var(v) => write!(f, "${}", v),
            ExpandsTo::Prim(p) => write!(f, "{}", p),
            ExpandsTo::IVar(v) => write!(f, "#{}", v),
        }
    }
}

/// a list of znodes, representing a path through a tree (a zipper)
pub type Zip = Vec<ZNode>;
/// the index of the empty zid `[]` in the list of zippers
const EMPTY_ZID: ZId = 0;

/// an argument to an abstraction. `id` is the main field here, we can use
/// it to lookup the corresponding tree using egraph[id]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Arg {
    pub shifted_id: Id,
    pub unshifted_id: Id, // in case `id` was shifted to make it an arg not sure if this will end up being useful
    pub shift: i32,
    pub cost: i32,
    pub expands_to: ExpandsTo,
}

/// ExpandsTo from a &Lambda node. Returns None if this is
/// and IVar (which is not considered a node type) and crashes
/// on Programs node.
#[inline]
fn expands_to_of_node(node: &Lambda) -> ExpandsTo {
    match node {
        Lambda::Var(i) => ExpandsTo::Var(*i),
        Lambda::Prim(p) => {
            // if *p == Symbol::from("?#") {
            //     panic!("I still need to handle this") // todo
            // } else {
                ExpandsTo::Prim(*p)
            // }
        },
        Lambda::Lam(_) => ExpandsTo::Lam,
        Lambda::App(_) => ExpandsTo::App,
        Lambda::IVar(i) => ExpandsTo::IVar(*i),
        _ => unreachable!()
    }
}

/// Returns Some(ExpandsTo) for what we expect the hole to expand to to follow
/// the target, and returns None if we expect it to become a ?# argchoice.
fn tracked_expands_to(pattern: &Pattern, hole_zip: &Zip, shared: &SharedData) -> ExpandsTo {
    // apply the hole zipper to the original expr being tracked to get the subtree
    // this will expand into, then get the ExpandsTo of that
    let id = shared.tracking.as_ref().unwrap().expr
        .apply_zipper(&hole_zip).unwrap();
    match expands_to_of_node(shared.tracking.as_ref().unwrap().expr.get(id)) {
        ExpandsTo::IVar(i) => {
            // in the case where we're searching for an IVar we need to be robust to relabellings
            // since this doesn't have to be canonical. What we can do is we can look over
            // each ivar the the pattern has defined with a first zid in pattern.first_zid_of_ivar, and
            // if our expressions' zids_of_ivar[i] contains this zid then we know these two ivars
            // must correspond to each other in the pattern and the tracked expr and we can just return
            // the pattern version (`j` below).
            let zips = shared.tracking.as_ref().unwrap().zips_of_ivar[i as usize].clone();
            for labelled_zip in pattern.arg_zips.iter() {
                if zips.contains(&labelled_zip.zip) {
                    return ExpandsTo::IVar(labelled_zip.ivar as i32);
                }
            }
            // it's a new ivar that hasnt been used already so it must take on the next largest var number
            return ExpandsTo::IVar(pattern.arity as i32);
        }
        e => e
    }
}

/// The heap item used for heap-based worklists. Holds a pattern
#[derive(Debug,Clone, Eq, PartialEq)]
pub struct HeapItem {
    key: HeapKey,
    pattern: Pattern,
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.key.partial_cmp(&other.key)
    }
}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}
impl HeapItem {
    fn new(pattern: Pattern, shared: &SharedData) -> Self {
        HeapItem {
            // key: pattern.body_utility * pattern.match_locations.iter().map(|loc|num_paths_to_node[loc]).sum::<i32>(),
            key: shared.cfg.heap_choice.heap_choice(&pattern, shared),
            // system time is suuuper slow btw you want to do something else
            // key: std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_nanos() as i32,
            pattern
        }
    }
}


/// This is the multithread data locked during the critical section of the algorithm.
#[derive(Debug, Clone)]
pub struct CriticalMultithreadData {
    donelist: Vec<FinishedPattern>,
    worklist: BinaryHeap<HeapItem>,
    utility_pruning_cutoff: i32,
    active_threads: AHashSet<std::thread::ThreadId>, // list of threads currently holding worklist items
}

/// All the data shared among threads, mostly read-only
/// except for the mutexes
#[derive(Debug)]
pub struct SharedData {
    pub crit: Mutex<CriticalMultithreadData>,
    pub max_heapkey: Mutex<HeapKey>,
    // pub arg_of_zid_node: Vec<AHashMap<Id,Arg>>,
    pub treenodes: Vec<Id>,
    pub node_of_id: Vec<Lambda>,
    pub programs_node: Id,
    pub roots: Vec<Id>,
    // pub zids_of_node: AHashMap<Id,Vec<ZId>>,
    // pub zip_of_zid: Vec<Zip>,
    // pub zid_of_zip: AHashMap<Zip, ZId>,
    // pub extensions_of_zid: Vec<ZIdExtension>,
    // pub refinables_of_shifted_arg: AHashMap<Id,Vec<Id>>,
    // pub uses_of_zid_refinable_loc: AHashMap<(ZId,Id,Id),i32>,
    // pub uses_of_shifted_arg_refinement: AHashMap<Id,AHashMap<Id,usize>>,
    pub egraph: EGraph,
    pub num_paths_to_node: Vec<i32>,
    pub tasks_of_node: Vec<AHashSet<usize>>,
    pub cost_of_node_once: Vec<i32>,
    pub cost_of_node_all: Vec<i32>,
    pub free_vars_of_node: Vec<AHashSet<i32>>,
    pub shifted_of_id: Vec<Shifted>,
    pub init_cost: i32,
    pub stats: Mutex<Stats>,
    pub cfg: CompressionStepConfig,
    pub tracking: Option<Tracking>,
}

/// Used for debugging tracking information
#[derive(Debug)]
pub struct Tracking {
    expr: Expr,
    zips_of_ivar: Vec<Vec<Zip>>,
    refined: Option<Expr>,
}

impl CriticalMultithreadData {
    /// Create a new mutable multithread data struct with
    /// a worklist that just has a single hole on it
    fn new(donelist: Vec<FinishedPattern>, treenodes: &Vec<Id>, cost_of_node_all: &Vec<i32>, num_paths_to_node: &Vec<i32>, node_of_id: &Vec<Lambda>, cfg: &CompressionStepConfig) -> Self {
        // push an empty hole onto a new worklist
        // let mut worklist = BinaryHeap::new();
        // worklist.push(HeapItem::new(Pattern::single_hole(treenodes, cost_of_node_all, num_paths_to_node, node_of_id, cfg)));
        
        let mut res = CriticalMultithreadData {
            donelist,
            worklist: BinaryHeap::new(),
            utility_pruning_cutoff: 0,
            active_threads: AHashSet::new(),
        };
        res.update(cfg);
        res
    }
    /// sort the donelist by utility, truncate to cfg.inv_candidates, update 
    /// update utility_pruning_cutoff to be the lowest utility
    #[inline(never)]
    fn update(&mut self, cfg: &CompressionStepConfig) {
        // sort in decreasing order by utility primarily, and break ties using the argchoice zids (just in order to be deterministic!)
        // let old_best = self.donelist.first().map(|x|x.utility).unwrap_or(0);
        self.donelist.sort_unstable_by(|a,b| (b.utility,&b.pattern.arg_zips).cmp(&(a.utility,&a.pattern.arg_zips)));
        self.donelist.truncate(cfg.inv_candidates);
        // the cutoff is the lowest utility
        self.utility_pruning_cutoff = if cfg.no_opt_upper_bound { 0 } else { std::cmp::max(0,self.donelist.last().map(|x|x.utility).unwrap_or(0)) };
    }
}




/// a strategy for choosing worklist item to use next
#[derive(Debug, Clone, clap::ArgEnum, Serialize)]
pub enum HeapChoice {
    Random,
    DFS,
    BFS,
    MaxBound,
    MaxBodyLocations,
    LowArityHighBound,
}

#[derive(Debug, Clone, Serialize, PartialOrd, Ord, Hash, PartialEq, Eq)]
pub enum HeapKey {
    Int(i32),
    IntInt(i32,i32),
}

impl HeapChoice {
    fn init(&self) -> HeapKey {
        match self {
            _ => HeapKey::Int(0)
        }
    }
    fn heap_choice(&self, pattern: &Pattern, shared: &SharedData) -> HeapKey {
        match self {
            &HeapChoice::Random => {
                let mut rng = rand::thread_rng();
                HeapKey::Int(rng.gen())
            },
            &HeapChoice::DFS => {
                let mut lock = shared.max_heapkey.lock();
                let maxkey = lock.deref_mut();
                if let  HeapKey::Int(i) = *maxkey {
                    let key = HeapKey::Int(i+1);
                    *maxkey = key.clone();
                    key
                } else { unreachable!() }
            },
            &HeapChoice::BFS => {
                let mut lock = shared.max_heapkey.lock();
                let maxkey = lock.deref_mut();
                if let  HeapKey::Int(i) = *maxkey {
                    let key = HeapKey::Int(i-1);
                    *maxkey = key.clone();
                    key
                } else { unreachable!() }
            },
            &HeapChoice::MaxBound => {
                HeapKey::Int(pattern.utility_upper_bound)
            },
            // &HeapChoice::MaxBodyLocations => {
            //     HeapKey::Int(pattern.body_utility_no_refinement * pattern.match_locations.iter().map(|loc|shared.num_paths_to_node[loc]).sum::<i32>())
            // },
            // &HeapChoice::LowArityHighBound => {
            //     HeapKey::IntInt(pattern.first_zid_of_ivar.len() as i32, pattern.utility_upper_bound)
            // },
            _ => unimplemented!(),
        }
    }
}

/// At the end of the day we convert our Inventions into InventionExprs to make
/// them standalone without needing to carry the EGraph around to figure out what
/// the body Id points to.
#[derive(Debug, Clone)]
pub struct Invention {
    pub body: Expr, // invention body (not wrapped in lambdas)
    pub arity: usize,
    pub name: String,
}
impl Invention {
    pub fn new(body: Expr, arity: usize, name: &str) -> Self {
        Self { body, arity, name: String::from(name) }
    }
    /// replace any #i with args[i], returning a new expression
    pub fn apply(&self, args: &[Expr]) -> Expr {
        assert_eq!(args.len(), self.arity);
        let map: AHashMap<i32, Expr> = args.iter().enumerate().map(|(i,e)| (i as i32, e.clone())).collect();
        ivar_replace(&self.body, self.body.root(), &map)
    }
}

impl Display for Invention {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "[{} arity={}: {}]", self.name, self.arity, self.body)
    }
}

/// A node in an ZPath
/// Ord: Func < Body < Arg
#[derive(Debug, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum ZNode {
    // * order of variants here is important because the derived Ord will use it
    Func, // zipper went into the function, so Id is the arg
    Body, 
    Arg, // zipper went into the arg, so Id is the function
}

/// "zipper id" each unique zipper gets referred to by its zipper id
pub type ZId = usize;

/// a zid referencing a specific ZPath and a #i index
#[derive(Debug,Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
struct LabelledZId {
    zid: ZId,
    ivar: usize // which #i argument this is, which also corresponds to args[i] ofc
}

/// a zid referencing a specific ZPath and a #i index
#[derive(Debug,Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct LabelledZip {
    pub zip: Zip,
    pub ivar: usize // which #i argument this is, which also corresponds to args[i] ofc
}

impl LabelledZip {
    pub fn new(zip: Zip, ivar: usize) -> Self {
        Self { zip, ivar }
    }
}

/// Various tracking stats
#[derive(Clone,Default, Debug)]
pub struct Stats {
    worklist_steps: usize,
    finished: usize,
    calc_final_utility: usize,
    upper_bound_fired: usize,
    // conflict_upper_bound_fired: usize,
    free_vars_fired: usize,
    single_use_fired: usize,
    single_task_fired: usize,
    useless_abstract_fired: usize,
    force_multiuse_fired: usize,
}

/// a strategy for choosing which hole to expand next in a partial pattern
#[derive(Debug, Clone, clap::ArgEnum, Serialize)]
pub enum HoleChoice {
    Random,
    BreadthFirst,
    DepthFirst,
    MaxLargestSubset,
    HighEntropy,
    LowEntropy,
    MaxCost,
    MinCost,
    ManyGroups,
    FewGroups,
    FewApps,
}

impl HoleChoice {
    #[inline(never)]
    fn choose_hole(&self, pattern: &Pattern, shared: &SharedData) -> usize {
        if pattern.hole_zips.len() == 1 {
            return 0;
        }
        match self {
            &HoleChoice::BreadthFirst => 0,
            &HoleChoice::DepthFirst => pattern.hole_zips.len() - 1,
            &HoleChoice::Random => {
                let mut rng = rand::thread_rng();
                rng.gen_range(0..pattern.hole_zips.len())
            },
            // &HoleChoice::FewApps => {
            //     pattern.holes.iter().enumerate().map(|(hole_idx,hole_zid)|
            //         (hole_idx, pattern.match_locations.iter().filter(|loc|shared.arg_of_zid_node[*hole_zid][loc].expands_to == ExpandsTo::App).count()))
            //             .min_by_key(|x|x.1).unwrap().0
            // }
            // &HoleChoice::MaxCost => {
            //     pattern.holes.iter().enumerate().map(|(hole_idx,hole_zid)|
            //         (hole_idx, pattern.match_locations.iter().map(|loc|shared.arg_of_zid_node[*hole_zid][loc].cost).sum::<i32>()))
            //             .max_by_key(|x|x.1).unwrap().0
            // }
            // &HoleChoice::MinCost => {
            //     pattern.holes.iter().enumerate().map(|(hole_idx,hole_zid)|
            //         (hole_idx, pattern.match_locations.iter().map(|loc|shared.arg_of_zid_node[*hole_zid][loc].cost).sum::<i32>()))
            //             .min_by_key(|x|x.1).unwrap().0
            // }
            // &HoleChoice::MaxLargestSubset => {
            //     // todo warning this is extremely slow, partially bc of counts() but I think
            //     // mainly because where there are like dozens of holes doing all these lookups and clones and hashmaps is a LOT
            //     pattern.holes.iter().enumerate()
            //         .map(|(hole_idx,hole_zid)| (hole_idx, *pattern.match_locations.iter()
            //             .map(|loc| shared.arg_of_zid_node[*hole_zid][loc].expands_to.clone()).counts().values().max().unwrap())).max_by_key(|&(_,max_count)| max_count).unwrap().0
            // }
            _ => unimplemented!()
        }
    }
}

impl LabelledZId {
    fn new(zid: ZId, ivar: usize) -> LabelledZId {
        LabelledZId { zid: zid, ivar: ivar }
    }
}

/// tells you which zid if any you would get if you extended the depth
/// (of whatever the current zid is) with any of these znodes.
#[derive(Clone,Debug)]
pub struct ZIdExtension {
    body: Option<ZId>,
    arg: Option<ZId>,
    func: Option<ZId>,
}

#[inline(never)]
fn donelist_update(
    donelist_buf: &mut Vec<FinishedPattern>,
    shared: &Arc<SharedData>,
) -> i32 {
    let mut shared_guard = shared.crit.lock();
    let mut crit: &mut CriticalMultithreadData = shared_guard.deref_mut();
    let old_best_utility = crit.donelist.first().map(|x|x.utility).unwrap_or(0);
    let old_donelist_len = crit.donelist.len();
    let old_utility_pruning_cutoff = crit.utility_pruning_cutoff;
    // drain from donelist_buf into the actual donelist
    crit.donelist.extend(donelist_buf.drain(..).filter(|done| done.utility > old_utility_pruning_cutoff));
    if !shared.cfg.no_stats { shared.stats.lock().deref_mut().finished += crit.donelist.len() - old_donelist_len; };
    // sort + truncate + update utility_pruning_cutoff
    crit.update(&shared.cfg); // this also updates utility_pruning_cutoff

    if shared.cfg.verbose_best && crit.donelist.first().map(|x|x.utility).unwrap_or(0) > old_best_utility {
        // println!("{} @ step={} util={} for {}", "[new best utility]".blue(), shared.stats.lock().deref_mut().worklist_steps, crit.donelist.first().unwrap().utility, crit.donelist.first().unwrap().info(shared));
    }

    // pull out the newer version of this now that its been updated, since we're returning it at the end
    let mut utility_pruning_cutoff = crit.utility_pruning_cutoff;
    utility_pruning_cutoff
}

/// empties worklist_buf and donelist_buf into the shared worklist while holding the mutex, updates
/// the donelist and cutoffs, and grabs and returns a new worklist item along with new cutoff bounds.
#[inline(never)]
fn get_worklist_item(
    worklist_buf: &mut Vec<HeapItem>,
    donelist_buf: &mut Vec<FinishedPattern>,
    shared: &Arc<SharedData>,
) -> Option<(Vec<Pattern>,i32)> {

    // * MULTITHREADING: CRITICAL SECTION START *
    // take the lock, which will be released immediately when this scope exits
    let mut shared_guard = shared.crit.lock();
    let mut crit: &mut CriticalMultithreadData = shared_guard.deref_mut();
    let old_best_utility = crit.donelist.first().map(|x|x.utility).unwrap_or(0);
    let old_donelist_len = crit.donelist.len();
    let old_utility_pruning_cutoff = crit.utility_pruning_cutoff;
    // drain from donelist_buf into the actual donelist
    crit.donelist.extend(donelist_buf.drain(..).filter(|done| done.utility > old_utility_pruning_cutoff));
    if !shared.cfg.no_stats { shared.stats.lock().deref_mut().finished += crit.donelist.len() - old_donelist_len; };
    // sort + truncate + update utility_pruning_cutoff
    crit.update(&shared.cfg); // this also updates utility_pruning_cutoff

    if shared.cfg.verbose_best && crit.donelist.first().map(|x|x.utility).unwrap_or(0) > old_best_utility {
        // println!("{} @ step={} util={} for {}", "[new best utility]".blue(), shared.stats.lock().deref_mut().worklist_steps, crit.donelist.first().unwrap().utility, crit.donelist.first().unwrap().info(shared));
    }

    // pull out the newer version of this now that its been updated, since we're returning it at the end
    let mut utility_pruning_cutoff = crit.utility_pruning_cutoff;

    let old_worklist_len = crit.worklist.len();
    let worklist_buf_len = worklist_buf.len();
    // drain from worklist_buf into the actual worklist
    crit.worklist.extend(worklist_buf.drain(..).filter(|heap_item| heap_item.pattern.utility_upper_bound > utility_pruning_cutoff));
    // num pruned by upper bound = num we were gonna add minus change in worklist length
    if !shared.cfg.no_stats { shared.stats.lock().deref_mut().upper_bound_fired += worklist_buf_len - (crit.worklist.len() - old_worklist_len); };

    let mut returned_items = vec![];

    // try to get a new worklist item
    crit.active_threads.remove(&thread::current().id()); // remove ourself from the active threads
    // println!("worklist len: {}", crit.worklist.len());

    loop {
        // with dynamic batch size, take worklist_size/num_threads items from the worklist
        let batch_size = if shared.cfg.dynamic_batch { std::cmp::max(1, crit.worklist.len() / shared.cfg.threads ) } else { shared.cfg.batch };
        while crit.worklist.is_empty() {
            if !returned_items.is_empty() {
                // give up and return whatever we've got
                crit.active_threads.insert(thread::current().id());
                return Some((returned_items, utility_pruning_cutoff));
            }
            if crit.active_threads.is_empty() {
                return None // all threads are stuck waiting for work so we're all done
            }
            // the worklist is empty but someone else currently has a worklist item so we should give up our lock then take it back
            drop(shared_guard);
            shared_guard = shared.crit.lock();
            crit = shared_guard.deref_mut();
            // update our cutoff in case it changed
            utility_pruning_cutoff = crit.utility_pruning_cutoff;
        }
        
        let heap_item = crit.worklist.pop().unwrap();
        // prune if upper bound is too low (cutoff may have increased in the time since this was added to the worklist)
        if shared.cfg.no_opt_upper_bound || heap_item.pattern.utility_upper_bound > utility_pruning_cutoff {
            // we got one!
            returned_items.push(heap_item.pattern);
            if returned_items.len() == batch_size {
                // we got enough, so return it
                crit.active_threads.insert(thread::current().id());
                return Some((returned_items, utility_pruning_cutoff));
            }
        } else {
            if !shared.cfg.no_stats { shared.stats.lock().deref_mut().upper_bound_fired += 1; };
        }
    }
    // * MULTITHREADING: CRITICAL SECTION END *
}


#[derive(Debug)]
pub struct Instruction {
    offset: usize,
    len: usize,
    action: Action,
    idxs: Option<Vec<usize>>, // only used in multiuse IVar cacse
}

impl Instruction {
    fn new(offset: usize, len: usize, action: Action) -> Instruction {
        Instruction {
            offset,
            len,
            action,
            idxs: None,
        }
    }
    fn new_with_idxs(offset: usize, len: usize, action: Action, idxs: Option<Vec<usize>>) -> Instruction {
        Instruction {
            offset,
            len,
            action,
            idxs,
        }
    }
    fn undo_of(i: Instruction) -> Instruction {
        if let Action::Expansion(expands_to,_) = i.action {
            Instruction { action: Action::Undo(expands_to), ..i }
        } else { unreachable!() }
    }
    #[inline]
    fn bound(&self) -> i32 {
        if let Action::Expansion(_, bound) = self.action {
            return bound
        } else {
            panic!("Instruction::bound() called on non-expansion instruction");
        }
    }
}

#[derive(Debug)]
pub enum Action {
    Start,
    Expansion(ExpandsTo, i32), // expansion and bound
    Undo(ExpandsTo),
    SetHoleChoice(usize, Zip, i32), // hole_idx, hole_zip, hole_depth
}


/// The core top down branch and bound search
fn stitch_search(
    shared: Arc<SharedData>,
) {

    let mut match_locations: Vec<MatchLocation> = shared.treenodes.iter().map(|&id| MatchLocation::new(id)).collect();
    match_locations.sort_by_key(|m|m.id); // we assume match_locations is always sorted
    if shared.cfg.no_top_lambda {
        match_locations.retain(|m| expands_to_of_node(&shared.node_of_id[usize::from(m.id)]) != ExpandsTo::Lam);
    } 

    let mut pattern =  Pattern::single_hole(&shared, &match_locations[..]);
    let mut instructions: Vec<Instruction> = vec![Instruction::new(0, match_locations.len(), Action::Start)];

    let mut hole_idx: usize = 0;
    let mut hole_zip: Zip = vec![];
    let mut hole_depth: i32 = 0;

    let mut donelist_buf: Vec<FinishedPattern> = vec![];
    let mut weak_utility_pruning_cutoff = 0;

    loop {

        weak_utility_pruning_cutoff = donelist_update(&mut donelist_buf, &shared);

        let instruction = match instructions.pop() {
            Some(instruction) => instruction,
            None => { break }
        };

        println!("{} {:?} in {}", "Processing".yellow().bold(), instruction, pattern.to_expr(&shared));

        let mut offset = instruction.offset;
        let mut len = instruction.len;
        let mut locs = &mut match_locations[offset..offset+len];

        match &instruction.action {
            Action::Start => {
                // lets start
            }
            Action::Expansion(expands_to, bound) => {
                if *bound <= weak_utility_pruning_cutoff {
                    // todo copy over the printout and increment stats here
                    continue
                }
                match expands_to {
                    ExpandsTo::App => {
                        // push 2 to hole_zips
                        pattern.hole_zips.push(hole_zip.clone().into_iter().chain(once(ZNode::Func)).collect());
                        pattern.hole_zips.push(hole_zip.clone().into_iter().chain(once(ZNode::Arg)).collect());
                        // body += NONTERM
                        pattern.body_utility_no_refinement += COST_NONTERMINAL;
                        // push 2 holes to all locs
                        for loc in locs.iter_mut() {
                            let hole_id = loc.hole_unshifted_ids.remove(hole_idx);
                            loc.undo_hole_id.push(hole_id);
                            if let Lambda::App([f,x]) = shared.node_of_id[usize::from(hole_id)] {
                                loc.hole_unshifted_ids.push(f);
                                loc.hole_unshifted_ids.push(x);
                            } else { unreachable!() }
                        }
                    },
                    ExpandsTo::Lam => {
                        // push 1 to hole_zips
                        pattern.hole_zips.push(hole_zip.clone().into_iter().chain(once(ZNode::Body)).collect());
                        // body += NONTERM
                        pattern.body_utility_no_refinement += COST_NONTERMINAL;
                        // push 1 hole to all locs
                        for loc in locs.iter_mut() {
                            let hole_id = loc.hole_unshifted_ids.remove(hole_idx);
                            loc.undo_hole_id.push(hole_id);
                            if let Lambda::Lam([b]) = shared.node_of_id[usize::from(hole_id)] {
                                loc.hole_unshifted_ids.push(b);
                            } else { unreachable!() }
                        }
                    },
                    ExpandsTo::Var(_) | ExpandsTo::Prim(_) => {
                        pattern.body_utility_no_refinement += COST_TERMINAL;
                        for loc in locs.iter_mut() {
                            let hole_id = loc.hole_unshifted_ids.remove(hole_idx);
                            loc.undo_hole_id.push(hole_id);
                        }
                    },
                    ExpandsTo::IVar(i) => {
                        let new_var = instruction.idxs.is_none();
                        assert_eq!(new_var, *i as usize == pattern.arity, "{} {} {}", *i, pattern.arity, pattern.arg_zips.len());
                        if new_var {
                            pattern.refinements.push(None);
                            pattern.arity += 1;
                        } else {
                            // subsetting to the multiuse ivar indices and adjusting len and locs appropriately
                            let idxs = instruction.idxs.as_ref().unwrap();
                            select_indices(locs, idxs);
                            len = idxs.len();
                            locs = &mut locs[..len];
                            pattern.any_loc_id = locs[0].id;
                        }

                        for loc in locs.iter_mut() {
                            let hole_id = loc.hole_unshifted_ids.remove(hole_idx);
                            loc.undo_hole_id.push(hole_id);
                            if new_var {
                                let shifted_id = shared.shifted_of_id[usize::from(hole_id)].shifted_by(- hole_depth);
                                loc.arg_shifted_ids.push(shifted_id)
                            }
                        }
                        pattern.arg_zips.push(LabelledZip::new(hole_zip.clone(), *i as usize));
                    }
                }

                // push an undo instruction
                instructions.push(Instruction::undo_of(instruction));
                println!("Pushing {:?} in {}", instructions.last().unwrap(), pattern.to_expr(&shared));

                // save our old hole_idx and hole_zip. offset and len are intentionally unused.
                // note this only happens if holezips is nonempty
                if !pattern.hole_zips.is_empty() {
                    instructions.push(Instruction::new(0, 0, Action::SetHoleChoice(hole_idx, hole_zip.clone(), hole_depth)));
                    println!("Pushing {:?} in {}", instructions.last().unwrap(), pattern.to_expr(&shared));
                }

            }

            Action::Undo(expands_to) => {
                match expands_to {
                    ExpandsTo::App => {
                        pattern.hole_zips.truncate(pattern.hole_zips.len() - 2);
                        pattern.body_utility_no_refinement -= COST_NONTERMINAL;
                        for loc in locs {
                            loc.hole_unshifted_ids.insert(hole_idx, loc.undo_hole_id.pop().unwrap());
                            loc.hole_unshifted_ids.truncate(loc.hole_unshifted_ids.len() - 2);
                        }
                    },
                    ExpandsTo::Lam => {
                        pattern.hole_zips.truncate(pattern.hole_zips.len() - 1);
                        pattern.body_utility_no_refinement -= COST_NONTERMINAL;
                        for loc in locs {
                            loc.hole_unshifted_ids.insert(hole_idx, loc.undo_hole_id.pop().unwrap());
                            loc.hole_unshifted_ids.truncate(loc.hole_unshifted_ids.len() - 1);
                        }
                    },
                    ExpandsTo::Var(_) | ExpandsTo::Prim(_) => {
                        pattern.body_utility_no_refinement -= COST_TERMINAL;
                        for loc in locs {
                            loc.hole_unshifted_ids.insert(hole_idx, loc.undo_hole_id.pop().unwrap());
                        }
                    },
                    ExpandsTo::IVar(i) => {
                        let new_var = instruction.idxs.is_none();

                        if !new_var {
                            // adjust len and locs before we iterate locs so that our len bound is okay
                            let idxs = instruction.idxs.as_ref().unwrap();
                            len = idxs.len();
                        }

                        // here we iter only up to `len` which is everything in the new_var case and is just idxs.len()
                        // in the non-newvar case
                        for loc in locs[..len].iter_mut() {
                            loc.hole_unshifted_ids.insert(hole_idx, loc.undo_hole_id.pop().unwrap());
                            if new_var {
                                loc.arg_shifted_ids.pop();
                            }
                        }

                        // re-sort things by hole_idx expansion since we mangled it
                        // todo im pretty sure 
                        for loc in locs[..len].iter_mut() {
                            loc.cached_expands_to = expands_to_of_node(&shared.node_of_id[usize::from(loc.hole_unshifted_ids[hole_idx])]);
                        }
                        locs[..len].sort_unstable_by(|loc1,loc2| loc1.cached_expands_to.cmp(&loc2.cached_expands_to).then(loc1.id.cmp(&loc2.id)));                


                        if !new_var {
                            // now it's safe to re-run select_indices to undo it (since it is its own inverse)
                            let idxs = instruction.idxs.as_ref().unwrap();
                            select_indices(locs, idxs);
                        }
                        if new_var {
                            pattern.arity -= 1;
                            pattern.refinements.pop();
                        }
                        pattern.arg_zips.pop();
                    }
                }
                continue
            }

            Action::SetHoleChoice(old_hole_idx, old_hole_zip, old_hole_depth) => {
                assert!(offset == 0 && len == 0);
                // re-insert our current hole idx and hole zip
                pattern.hole_zips.insert(hole_idx, hole_zip);
                println!("hole zips after setholechoice: {:?}", pattern.hole_zips);
                // now set them to their old values
                hole_idx = *old_hole_idx;
                hole_zip = old_hole_zip.clone();
                hole_depth = *old_hole_depth;
                continue
            }
        }

        pattern.any_loc_id = locs[0].id;

        let tracked = false; // todo fix
        // we put these here because they need to come after the args have been  pointwise updated at every loc
        if opt_force_multiuse(&pattern, locs, tracked, &shared) { continue };
        if opt_useless_abstract(&pattern, locs, tracked, &shared) { continue };    

        if pattern.hole_zips.is_empty() {
            let tracked = false; // todo fix
            println!("{} {}", "Finishing".green().bold(), pattern.to_expr(&shared));
            finish_pattern(&mut pattern, locs, &mut weak_utility_pruning_cutoff, tracked, &mut donelist_buf, &shared);
            continue;
        }



        // if !shared.cfg.no_stats { shared.stats.lock().deref_mut().worklist_steps += 1; };
        // if !shared.cfg.no_stats { if shared.cfg.print_stats > 0 &&  shared.stats.lock().deref_mut().worklist_steps % shared.cfg.print_stats == 0 { println!("{:?} \n\t@ [bound={}; uses={}] chose: {}",shared.stats.lock().deref_mut(),   original_pattern.utility_upper_bound, original_pattern.match_locations.as_ref().unwrap().iter().map(|loc| shared.num_paths_to_node[usize::from(loc.id)]).sum::<i32>(), original_pattern.to_expr(&shared)); }};
        // if shared.cfg.verbose_worklist {
        //     println!("[bound={}; uses={}] chose: {}", original_pattern.utility_upper_bound, original_pattern.match_locations.as_ref().unwrap().iter().map(|loc| shared.num_paths_to_node[usize::from(loc.id)]).sum::<i32>(), original_pattern.to_expr(&shared));
        // }

        println!("hole zips before choice: {:?}", pattern.hole_zips);

        // choose our new hole_idx and hole_zip
        hole_idx = shared.cfg.hole_choice.choose_hole(&pattern, &shared);
        hole_zip = pattern.hole_zips.get(hole_idx).unwrap().clone();
        hole_depth = hole_zip.iter().filter(|z| **z == ZNode::Body).count() as i32;


        // println!("hole zips after choice: {:?}", pattern.hole_zips);

        let mut found_tracked = false;


        // sort the match locations by node type (ie what theyll expand into) so that we can do a group_by() on
        // node type in order to iterate over all the different expansions
        // We also sort secondarily by `loc` to ensure each groupby subsequence has the locations in sorted order
        for loc in locs.iter_mut() {
            loc.cached_expands_to = expands_to_of_node(&shared.node_of_id[usize::from(loc.hole_unshifted_ids[hole_idx])]);
        }
        locs.sort_unstable_by(|loc1,loc2| loc1.cached_expands_to.cmp(&loc2.cached_expands_to).then(loc1.id.cmp(&loc2.id)));
        
        // add all expansions to instructions, finishing anything that lacks holes

        // scoping for automatic drops so we dont accidentally use these variables later
        {
            let mut expands_to = &locs[0].cached_expands_to;
            let new_instructions_start = instructions.len();
            let mut inner_offset = 0;
            let mut inner_len = 0;
            for (i,loc) in locs.iter().enumerate() {
                if &loc.cached_expands_to != expands_to {
                    expand_and_finish(&mut pattern, &locs, None, inner_offset, inner_len, offset, expands_to, &hole_zip, hole_depth, &mut found_tracked, &mut weak_utility_pruning_cutoff, &mut instructions, &mut donelist_buf, &shared);
                    expands_to = &loc.cached_expands_to;
                    inner_offset = i;
                    inner_len = 0; // reset it to 0 which will immediately get incremented to 1
                }
                inner_len += 1;
            }
            // sort in increasing order so highest bound is at the end (top of stack)
            instructions[new_instructions_start..].sort_unstable_by_key(|instruction| instruction.bound());
        }
        

        let locs_of_ivar = get_locs_of_ivar(&pattern, &locs, hole_idx, hole_depth, &shared);

        // todo note you can keep a persistant copy of this around if you dont want all the allocs and just clone it at various points
        // todo note we have utility_upper_bound_single() if you want to check the utility without select_indices, however this wont help with the other kinds of pruning
        for (ivar,ivar_locs) in locs_of_ivar.into_iter().enumerate() {
            if ivar_locs.is_empty() { continue; }
            select_indices(locs, &ivar_locs);
            println!("old");
            let expands_to = ExpandsTo::IVar(ivar as i32);
            let inner_offset = 0;
            let inner_len = len; // here we pass in the full length since we need that to run select_indices when recursing
            expand_and_finish(&mut pattern, &locs, Some(ivar_locs.clone()), inner_offset, inner_len, offset, &expands_to, &hole_zip, hole_depth, &mut found_tracked, &mut weak_utility_pruning_cutoff, &mut instructions, &mut donelist_buf, &shared);
            // select_indices is its own inverse so we can restore the original ordering like this...
            select_indices(locs, &ivar_locs);
        }

        // add a new var if we have the arity for it
        if pattern.arity < shared.cfg.max_arity {
            // same offset and len as parent since it matches everywhere! And same bound!
            let expands_to = ExpandsTo::IVar(pattern.arity as i32);
            let inner_offset = 0;
            let inner_len = len;
            println!("new");
            expand_and_finish(&mut pattern, &locs, None, inner_offset, inner_len, offset, &expands_to, &hole_zip, hole_depth, &mut found_tracked, &mut weak_utility_pruning_cutoff, &mut instructions, &mut donelist_buf, &shared);
            // instructions.push(Instruction::new(offset, len, Action::Expansion(ExpandsTo::IVar(pattern.arity as i32), pattern.bound)));
        }



        if pattern.tracked && !found_tracked {
            println!("{} pruned when expanding because there were no match locations for the target expansion of {}", "[TRACK]".red().bold(), pattern.show_track_expansion(&hole_zip, &shared));
        }

        pattern.hole_zips.remove(hole_idx);
}

pub fn get_locs_of_ivar(pattern: &Pattern, locs: &[MatchLocation], hole_idx: usize, hole_depth: i32, shared :&SharedData) ->  Vec<Vec<usize>> {
    let mut locs_of_ivar: Vec<Vec<usize>> = (0..pattern.arity).map(|_| vec![]).collect();
    for (i,loc) in locs.iter().enumerate() {
        let unshifted_id = loc.hole_unshifted_ids[hole_idx];
        let shifted_id = shared.shifted_of_id[usize::from(unshifted_id)].shifted_by(-hole_depth);
        
        // reusing an old var
        for ivar in 0..pattern.arity {
            if shifted_id == loc.arg_shifted_ids[ivar] {
                locs_of_ivar[ivar].push(i)
            }
        }
    }
    locs_of_ivar
}



pub fn expand_and_finish(
    pattern: &mut Pattern,
    locs: &[MatchLocation],
    ivar_locs: Option<Vec<usize>>,
    inner_offset: usize,
    inner_len: usize,
    parent_offset: usize,
    expands_to: &ExpandsTo,
    hole_zip: &Zip,
    hole_depth: i32,
    found_tracked: &mut bool,
    weak_utility_pruning_cutoff: &mut i32,
    instructions: &mut Vec<Instruction>,
    donelist_buf: &mut Vec<FinishedPattern>,
    shared: &SharedData) {


    let inner_locs = &locs[inner_offset..inner_offset+inner_len];

    // for debugging
    let tracked = pattern.tracked && *expands_to == tracked_expands_to(&pattern, &hole_zip, &shared);
    if tracked { *found_tracked = true; }
    if shared.cfg.follow_track && !tracked { return }
    
    if opt_single_use(&pattern, inner_locs, hole_zip, expands_to, tracked, &shared) { return };
    if opt_single_task(&pattern, inner_locs, hole_zip, expands_to, tracked, &shared) { return  };

    // check for free variables: if an invention has free variables in the body then it's not a real function and we can discard it
    // Here we just check if our expansion just yielded a variable, and if that is bound based on how many lambdas there are above it.
    if let ExpandsTo::Var(i) = expands_to {
        if *i >= hole_depth {
            if !shared.cfg.no_stats { shared.stats.lock().deref_mut().free_vars_fired += 1; };
            if tracked { println!("{} pruned by free var in body when expanding to {}", "[TRACK]".red().bold(), pattern.show_track_expansion(&hole_zip, &shared)); }
            return; // free var
        }
    }

    // update the upper bound
    let util_upper_bound: i32 = utility_upper_bound(&inner_locs, pattern.body_utility_no_refinement, &shared);
    assert!(util_upper_bound <= pattern.utility_upper_bound);
    println!("expand and finish on {:?}", expands_to);

    if opt_upper_bound(&pattern,  util_upper_bound, *weak_utility_pruning_cutoff, hole_zip, expands_to, tracked, &shared) { return };

    if tracked { println!("{} pushed {} to work list (bound: {})", "[TRACK]".green().bold(), pattern.show_track_expansion(&hole_zip, &shared), util_upper_bound); }

    // if !pattern.hole_zips.is_empty() || expands_to.has_holes() {
    //     println!("pushin {:?}", expands_to);
    let mut instruction = Instruction::new_with_idxs(parent_offset + inner_offset, inner_len, Action::Expansion(expands_to.clone(), util_upper_bound), ivar_locs);
    instructions.push(instruction);
    println!("Pushing {:?} in {}", instructions.last().unwrap(), pattern.to_expr(shared));
    // } else {
    //     println!("finishin {}", pattern.to_expr(shared));
    // }
}


/// refines a new pattern inplace
#[inline(never)]
pub fn refine(new_pattern: &mut Pattern, inner_locs: &[MatchLocation], tracked: bool, shared: &SharedData) {
    if tracked {
        println!("{} refining {}", "[TRACK:REFINE]".yellow().bold(), new_pattern.to_expr(&shared));
    }

    let mut best_refinement: Vec<Option<Vec<Id>>> = new_pattern.refinements.clone(); // initially all Nones
    let mut best_utility = noncompressive_utility(new_pattern.body_utility_no_refinement + new_pattern.refinement_body_utility, &shared.cfg) + compressive_utility(&new_pattern, inner_locs, &shared).util;
    let mut best_refinement_body_utility = 0;
    assert!(new_pattern.refinement_body_utility == 0);
    assert!(new_pattern.refinements.iter().all(|refinement| refinement.is_none()));

    // get all refinement options for each arg, deduped
    let mut refinements_by_arg: Vec<Vec<Id>> = unimplemented!();
    // todo old code:
    // let mut refinements_by_arg: Vec<Vec<Id>> = (0..new_pattern.arity).map(|ivar| 
    //     new_pattern.match_locations.iter().flat_map(|loc|
    //         shared.uses_of_shifted_arg_refinement.get(&loc.arg_shifted_ids[ivar]).map(|uses_of_refinement| uses_of_refinement.keys())
    //     ).flatten().cloned().collect::<AHashSet<_>>().into_iter().collect()).collect();

    for ivar in 0..new_pattern.arity { // for each arg
        if new_pattern.arg_zips.iter().filter(|l |l.ivar == ivar).count() > 1 {
            refinements_by_arg[ivar] = Vec::new(); // todo limitation: we dont refine multiuse
        }
    }

    let mut num_refinements = 0;

    'refinements: for refinements in refinements_by_arg.into_iter()
        .map(|refinements|
                (1..=shared.cfg.max_refinement_arity).map(move |k| refinements.clone().into_iter()
                    .combinations(k))
                .flatten()
                .map(|r| Some(r))
                .chain(std::iter::once(None))
                )
        .multi_cartesian_product()
    {
        num_refinements += 1;
        // insert the refinement
        new_pattern.refinements = refinements.clone();
        // body grows by an APP and the refined out subtree's size
        new_pattern.refinement_body_utility = refinements.iter()
            .flat_map(|r| r)
            .map(|r| r.iter()
                .map(|r_id| COST_NONTERMINAL + shared.cost_of_node_once[usize::from(*r_id)]).sum::<i32>()).sum::<i32>();

        let utility = noncompressive_utility(new_pattern.body_utility_no_refinement + new_pattern.refinement_body_utility, &shared.cfg) + compressive_utility(&new_pattern,inner_locs, &shared).util;
        if utility > best_utility {
            best_refinement = refinements.clone();
            best_utility = utility;
            best_refinement_body_utility = new_pattern.refinement_body_utility;
        }
        if tracked {
            println!("{} refined to {} (util: {})", "[TRACK:REFINE]".yellow().bold(), new_pattern.to_expr(&shared), utility);
            println!("{:?}", refinements);
            if let Some(track_refined) = &shared.tracking.as_ref().unwrap().refined {
                let refined = new_pattern.to_expr(&shared).to_string();
                let track_refined = track_refined.to_string();
                if refined == track_refined {
                    println!("{} previous refinement was the tracked one! Forcing it to accept that one", "[TRACK:REFINE]".green().bold());
                    best_refinement = refinements.clone();
                    best_refinement_body_utility = new_pattern.refinement_body_utility;
                    new_pattern.refinement_body_utility = 0;
                    break 'refinements;
                }
            }
        }
        // reset body utility
        new_pattern.refinement_body_utility = 0;
    }
    if num_refinements > 1000 {
        println!("[many refinements] tried {} refinements for {}", num_refinements, new_pattern.to_expr(&shared));
    }

    // set to the best
    new_pattern.refinements = best_refinement.clone();
    new_pattern.refinement_body_utility = best_refinement_body_utility;
}


fn opt_single_use(pattern: &Pattern, locs: &[MatchLocation], hole_zip: &Zip, expands_to: &ExpandsTo, tracked: bool, shared: &SharedData) -> bool {
    // prune inventions that only match at a single unique (structurally hashed) subtree. This only applies if we
    // also are priming with arity 0 inventions. Basically if something only matches at one subtree then the best you can
    // do is the arity zero invention which is the whole subtree, and since we already primed with arity 0 inventions we can
    // prune here. The exception is when there are free variables so arity 0 wouldn't have applied.
    // Also, note that upper bounding + arity 0 priming does nearly perfectly handle this already, but there are cases where
    // you can't improve your structure penalty bound enough to catch everything hence this separate single_use thing.
    if !shared.cfg.no_opt_single_use && !shared.cfg.no_opt_arity_zero && locs.len()  == 1 && shared.free_vars_of_node[usize::from(locs[0].id)].is_empty() {
        if !shared.cfg.no_stats { shared.stats.lock().deref_mut().single_use_fired += 1; }
        if tracked { println!("{} single use pruned when expanding to {}", "[TRACK]".red().bold(), pattern.show_track_expansion(&hole_zip, &shared)); }
        return true
    }
    return false
}

fn opt_single_task(pattern: &Pattern, locs: &[MatchLocation], hole_zip: &Zip, expands_to: &ExpandsTo, tracked: bool, shared: &SharedData) -> bool {
    // prune inventions specific to one single task
    if !shared.cfg.no_opt_single_task
            && locs.iter().all(|loc| shared.tasks_of_node[usize::from(loc.id)].len() == 1)
            && locs.iter().all(|loc| shared.tasks_of_node[usize::from(locs[0].id)].iter().next() == shared.tasks_of_node[usize::from(loc.id)].iter().next()) {
        if !shared.cfg.no_stats { shared.stats.lock().deref_mut().single_task_fired += 1; }
        if tracked { println!("{} single task pruned when expanding to {}", "[TRACK]".red().bold(), pattern.show_track_expansion(&hole_zip, &shared)); }
        return true
    }
    return false
}

fn opt_upper_bound(pattern: &Pattern, util_upper_bound: i32, weak_utility_pruning_cutoff: i32, hole_zip: &Zip, expands_to: &ExpandsTo, tracked: bool, shared: &SharedData) -> bool {
    // branch and bound: if the upper bound is less than the best invention we've found so far (our cutoff), we can discard this pattern
    if !shared.cfg.no_opt_upper_bound && util_upper_bound <= weak_utility_pruning_cutoff {
        if !shared.cfg.no_stats { shared.stats.lock().deref_mut().upper_bound_fired += 1; };
        if tracked { println!("{} upper bound ({} < {}) pruned when expanding {}", "[TRACK]".red().bold(), util_upper_bound, weak_utility_pruning_cutoff, pattern.show_track_expansion(&hole_zip, &shared)); }
        return true // too low utility
    }
    return false
}

fn opt_force_multiuse(pattern: &Pattern, locs: &[MatchLocation], tracked: bool, shared: &SharedData) -> bool {
    // if two different ivars #i and #j have the same arg at every location, then we can prune this pattern
    // because there must exist another pattern where theyre just both the same ivar. Note that this pruning
    // happens here and not just at the ivar creation point because new subsetting can happen
    if !shared.cfg.no_opt_force_multiuse {
        // for all pairs of ivars #i and #j, get the first zipper and compare the arg value across all locations
        for i in (0..pattern.arity) {
            // let ref arg_of_loc_1 = shared.arg_of_zid_node[*ivar_zid_1];
            for j in (i+1..pattern.arity) {
                // let ref arg_of_loc_2 = shared.arg_of_zid_node[*ivar_zid_2];
                if locs.iter().all(|loc|
                    loc.arg_shifted_ids[i] == loc.arg_shifted_ids[j])
                    // arg_of_loc_1[loc].shifted_id == arg_of_loc_2[loc].shifted_id)
                {
                    if !shared.cfg.no_stats { shared.stats.lock().deref_mut().force_multiuse_fired += 1; };
                    if tracked { println!("{} force multiuse pruned when expanding to {}", "[TRACK]".red().bold(), pattern.to_expr(&shared)); }
                    return true
                }
            }
        }
    }
    return false
}

fn opt_useless_abstract(pattern: &Pattern, locs: &[MatchLocation], tracked: bool, shared: &SharedData) -> bool {
    // check for useless abstractions (ie ones that take the same arg everywhere). We check for this all the time, not just when adding a new variables,
    // because subsetting of match_locations can turn previously useful abstractions into useless ones.
    if !shared.cfg.no_opt_useless_abstract {
        for ivar in 0..pattern.arity{
            // if its the same arg in every place
            if locs.iter().map(|loc| &loc.arg_shifted_ids[ivar]).all_equal()
                // AND there's no potential for refining that arg
                && (!shared.cfg.refine || locs.iter().all(|loc| shared.free_vars_of_node[usize::from(loc.arg_shifted_ids[ivar].downshifted_id)].is_empty())) // safe: emptiness of free vars is equiv to emptiness of downshifted free vars
            {
                if !shared.cfg.no_stats { shared.stats.lock().deref_mut().useless_abstract_fired += 1; };
                return true // useless abstraction
            }
        }

    }
    return false
}

fn finish_pattern(pattern: &mut Pattern, inner_locs: &[MatchLocation], weak_utility_pruning_cutoff: &mut i32, tracked:bool, donelist_buf: &mut Vec<FinishedPattern>, shared: &SharedData) {
    assert!(pattern.hole_zips.is_empty());
    // it's a finished pattern
    // refinement

    if shared.cfg.refine {
        refine(pattern, inner_locs, tracked, &shared);
    }

    // todo add a conflict-free check first i think would be good?
    let util_calc = compressive_utility(&pattern, inner_locs, shared);
    let noncompressive_utility = noncompressive_utility(pattern.body_utility_no_refinement + pattern.refinement_body_utility, &shared.cfg);
    let utility = noncompressive_utility + util_calc.util;
    assert!(utility <= pattern.utility_upper_bound, "{} BUT utility is higher: {}", pattern.info(&shared, inner_locs), utility);

    if utility <= *weak_utility_pruning_cutoff {
        if !shared.cfg.no_stats { shared.stats.lock().deref_mut().upper_bound_fired += 1; };
        if tracked { println!("{} upper bound (stage 2) ({} < {}) pruned on {}", "[TRACK]".red().bold(), utility, weak_utility_pruning_cutoff, pattern.to_expr(&shared)); }
        return
    }

    if shared.cfg.inv_candidates == 1 && utility > *weak_utility_pruning_cutoff {
        // if we're only looking for one invention, we can directly update our cutoff here
        *weak_utility_pruning_cutoff = utility;
    }

    let finished_pattern = FinishedPattern::new(pattern, inner_locs, utility, util_calc.util, util_calc, &shared);

    if !shared.cfg.no_stats { shared.stats.lock().calc_final_utility += 1; };

    if shared.cfg.rewrite_check {
        // run rewriting just to make sure the assert in it passes
        rewrite_fast(&finished_pattern, &shared, &"fake_inv");
    }

    if tracked {
        println!("{} pushed {} to donelist (util: {})", "[TRACK:DONE]".green().bold(), finished_pattern.to_expr(&shared), finished_pattern.utility);
    }

    donelist_buf.push(finished_pattern);

    }
}



/// A finished invention
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedPattern {
    pub pattern: Pattern,
    pub match_locations: Vec<MatchLocation>, // todo note that this doesnt need to be carried around in here! we could just reconstruct it later by a directed search
    pub utility: i32,
    pub compressive_utility: i32,
    pub util_calc: UtilityCalculation,
    pub arity: usize,
    pub usages: i32,
}

impl FinishedPattern {
    #[inline(never)]
    fn new(pattern: &Pattern, match_locations: &[MatchLocation], utility: i32, compressive_utility: i32, util_calc: UtilityCalculation, shared: &SharedData) -> Self {
        let arity = pattern.arity;
        let usages = match_locations.iter().map(|loc| shared.num_paths_to_node[usize::from(loc.id)]).sum();
        FinishedPattern {
            pattern: pattern.clone(),
            match_locations: match_locations.to_vec(), // todo expensive! and unnecessary  we could just reconstruct it later
            utility,
            compressive_utility,
            util_calc,
            arity,
            usages,
        }
    }
    // convert finished invention to an Expr
    pub fn to_expr(&self, shared: &SharedData) -> Expr {
        self.pattern.to_expr(shared)
    }
    pub fn to_invention(&self, name: &str, shared: &SharedData) -> Invention {
        Invention::new(self.to_expr(shared), self.arity, name)
    }
    pub fn info(&self, shared: &SharedData) -> String {
        format!("{} -> finished: utility={}, compressive_utility={}, arity={}, usages={}",self.pattern.info(shared, &self.match_locations[..]), self.utility, self.compressive_utility, self.arity, self.usages)
    }

}
// #[derive(Debug, Clone, PartialEq, Eq, Hash)]
// struct Refinement {
//     refined_subtree: Id, // the thing you can refine out
//     uses: HashMap<Id,i32>, // map from loc to number of times it's used
//     refined_subtree_cost: i32, // the compressive utility gained by refining it
// }

/// return all possible refinements you could use here along with counts for how many of each you can use
fn get_refinements_of_shifted_id(shifted_id: Id, egraph: &crate::EGraph, cfg: &CompressionStepConfig) -> AHashMap<Id,usize>
{
    fn helper(id: Id, egraph: &crate::EGraph, cfg: &CompressionStepConfig, refinements: &mut Vec<Id>) {
        let ivars =  egraph[id].data.free_ivars.len();
        if ivars == 0 {
            return; // todo limitation: we dont thread things that dont have ivars, for sortof good reasons.
        }
        
        if !egraph[id].data.free_vars.is_empty() {
            return; // if something has free vars and theyre not turned into ivars they must either be refs to lambdas within the arg or refs ABOVE the invention which in either case we can't refine
        }

        if egraph[id].data.inventionless_cost <= cfg.max_refinement_size.unwrap_or(i32::MAX) {
            refinements.push(id);
        }

        // ivar!
        for child in egraph[id].nodes[0].children().iter() {
            helper(*child, egraph, cfg, refinements);
        }
    }
    let mut refinements = vec![];
    helper(shifted_id, egraph, cfg, &mut refinements);
    counts_ahash(&refinements)
}

/// figure out all the N^2 zippers from choosing any given node and then choosing a descendant and returning the zipper from
/// the node to the descendant. We also collect a bunch of other useful stuff like the argument you would get if you abstracted
/// the descendant and introduced an invention rooted at the ancestor node.
fn get_zippers(
    treenodes: &[Id],
    cost_of_node_once: &Vec<i32>,
    no_cache: bool,
    egraph: &mut crate::EGraph,
    cfg: &CompressionStepConfig
) -> (AHashMap<Zip, ZId>, Vec<Zip>, Vec<AHashMap<Id,Arg>>, AHashMap<Id,Vec<ZId>>,  Vec<ZIdExtension>, AHashMap<Id,AHashMap<Id,usize>>) {
    let cache: &mut Option<RecVarModCache> = &mut if no_cache { None } else { Some(AHashMap::new()) };

    let mut zid_of_zip: AHashMap<Zip, ZId> = Default::default();
    let mut zip_of_zid: Vec<Zip> = Default::default();
    let mut arg_of_zid_node: Vec<AHashMap<Id,Arg>> = Default::default();
    let mut zids_of_node: AHashMap<Id,Vec<ZId>> = Default::default();


    // let mut refinements_of_shifted_arg: AHashMap<Id,AHashSet<Id>> = Default::default();
    // let mut uses_of_zid_refinable_loc: AHashMap<(ZId,Id,Id),i32> = Default::default();
    let mut uses_of_shifted_arg_refinement: AHashMap<Id,AHashMap<Id,usize>> = Default::default();


    zid_of_zip.insert(vec![], EMPTY_ZID);
    zip_of_zid.push(vec![]);
    arg_of_zid_node.push(AHashMap::new());
    assert!(EMPTY_ZID == 0);
    
    // loop over all nodes in all programs in bottom up order
    for treenode in treenodes.iter() {
        // println!("processing id={}: {}", treenode, extract(*treenode, egraph) );

        // im essentially using the egraph just for its structural hashing rn
        assert!(egraph[*treenode].nodes.len() == 1);
        // clone to appease the borrow checker
        let node = egraph[*treenode].nodes[0].clone();
        
        // any node can become the identity function (the empty zipper with itself as the arg)
        let mut zids: Vec<ZId> = vec![EMPTY_ZID];
        arg_of_zid_node[EMPTY_ZID].insert(*treenode,
            Arg { shifted_id: *treenode, unshifted_id: *treenode, shift: 0, cost: cost_of_node_once[usize::from(*treenode)], expands_to: expands_to_of_node(&node) });
        
        match node {
            Lambda::IVar(_) => { panic!("attempted to abstract an IVar") }
            Lambda::Var(_) | Lambda::Prim(_) | Lambda::Programs(_) => {},
            Lambda::App([f,x]) => {
                // bubble from `f`
                for f_zid in zids_of_node[&f].iter() {
                    // clone and extend zip to get new zid for this node
                    let mut zip = zip_of_zid[*f_zid].clone();
                    zip.insert(0,ZNode::Func);
                    let zid = zid_of_zip.entry(zip.clone()).or_insert_with(|| {
                        let zid = zip_of_zid.len();
                        zip_of_zid.push(zip);
                        arg_of_zid_node.push(AHashMap::new());
                        zid
                    });
                    // add new zid to this node
                    zids.push(*zid);
                    // give it the same arg
                    let arg = arg_of_zid_node[*f_zid][&f].clone();
                    arg_of_zid_node[*zid].insert(*treenode, arg);
                }

                // bubble from `x`
                for x_zid in zids_of_node[&x].iter() {
                    // clone and extend zip to get new zid for this node
                    let mut zip = zip_of_zid[*x_zid].clone();
                    zip.insert(0,ZNode::Arg);
                    let zid = zid_of_zip.entry(zip.clone()).or_insert_with(|| {
                        let zid = zip_of_zid.len();
                        zip_of_zid.push(zip);
                        arg_of_zid_node.push(AHashMap::new());
                        zid
                    });
                    // add new zid to this node
                    zids.push(*zid);
                    // give it the same arg
                    let arg = arg_of_zid_node[*x_zid][&x].clone();
                    arg_of_zid_node[*zid].insert(*treenode, arg);

                }
            },
            Lambda::Lam([b]) => {
                for b_zid in zids_of_node[&b].iter() {

                    // clone and extend zip to get new zid for this node
                    let mut zip = zip_of_zid[*b_zid].clone();
                    zip.insert(0,ZNode::Body);
                    let zid = zid_of_zip.entry(zip.clone()).or_insert_with(|| {
                        let zid = zip_of_zid.len();
                        zip_of_zid.push(zip.clone());
                        arg_of_zid_node.push(AHashMap::new());
                        zid
                    });
                    // add new zid to this node
                    zids.push(*zid);
                    // shift the arg but keep the unshifted part the same
                    let mut arg: Arg = arg_of_zid_node[*b_zid][&b].clone();

                    if !egraph[arg.shifted_id].data.free_vars.is_empty() {
                        // println!("stepping from child: {}", extract(b, egraph));
                        // println!("stepping to parent : {}", extract(*treenode, egraph));
                        // println!("b_zid: {}; b_zip: {:?}", b_zid, zip_of_zid[*b_zid]);
                        // println!("shift from: {}", extract(arg.id, egraph));
                        // println!("shift to:   {}", extract(arg.id, egraph));
                        // println!("total shift: {}", arg.shift);
                        if egraph[arg.shifted_id].data.free_vars.contains(&0) {
                            // we  go one less than the depth from the root to the arg. That way $0 when we're hopping
                            // the only  lambda in existence will map to depth_root_to_arg-1 = 1-1 = 0 -> #0 which will then
                            // be transformed back #0 -> $0 + depth = $0 + 0 = $0 if we thread it directly for example.
                            let depth_root_to_arg = zip.iter().filter(|x| **x == ZNode::Body).count() as i32;
                            arg.shifted_id = insert_arg_ivars(arg.shifted_id, depth_root_to_arg-1, egraph).unwrap();
                        }
                        arg.shifted_id = shift(arg.shifted_id, -1, egraph, cache).unwrap();
                        arg.shift -= 1;
                        if cfg.refine {
                            // refinements:
                            if !uses_of_shifted_arg_refinement.contains_key(&arg.shifted_id) {
                                uses_of_shifted_arg_refinement.insert(arg.shifted_id,get_refinements_of_shifted_id(arg.shifted_id, &egraph, cfg));
                            }
                            // refinements_of_shifted_arg.entry(arg.shifted_id).or_default().extend(refinement_counts.keys().cloned());
                            // uses_of_zid_refinable_loc
                        }
                    }
                    arg_of_zid_node[*zid].insert(*treenode, arg);
                }            },
        }
        zids_of_node.insert(*treenode, zids);
    }

    let extensions_of_zid = zip_of_zid.iter().map(|zip| {
        let mut zip_body = zip.clone();
        zip_body.push(ZNode::Body);
        let mut zip_arg = zip.clone();
        zip_arg.push(ZNode::Arg);
        let mut zip_func = zip.clone();
        zip_func.push(ZNode::Func);
        ZIdExtension {
            body: zid_of_zip.get(&zip_body).copied(),
            arg: zid_of_zip.get(&zip_arg).copied(),
            func: zid_of_zip.get(&zip_func).copied(),
        }
    }).collect();

    (zid_of_zip,
    zip_of_zid,
    arg_of_zid_node,
    zids_of_node,
    extensions_of_zid,
    uses_of_shifted_arg_refinement)
}

/// the complete result of a single step of compression, this is a somewhat expensive data structure
/// to create.
#[derive(Debug, Clone)]
pub struct CompressionStepResult {
    pub inv: Invention,
    pub rewritten: Expr,
    pub rewritten_dreamcoder: Vec<String>,
    pub done: FinishedPattern,
    pub expected_cost: i32,
    pub final_cost: i32,
    pub multiplier: f64,
    pub multiplier_wrt_orig: f64,
    pub uses: i32,
    pub use_exprs: Vec<Expr>,
    pub use_args: Vec<Vec<Expr>>,
    pub dc_inv_str: String,
    pub initial_cost: i32,
}

impl CompressionStepResult {
    fn new(done: FinishedPattern, inv_name: &str, shared: &mut SharedData, past_invs: &Vec<CompressionStepResult>) -> Self {

        // cost of the very first initial program before any inventions
        let very_first_cost = if let Some(past_inv) = past_invs.first() { past_inv.initial_cost } else { shared.init_cost };

        let inv = done.to_invention(inv_name, shared);
        let rewritten = Expr::programs(rewrite_fast(&done, &shared, &inv.name));


        let expected_cost = shared.init_cost - done.compressive_utility;
        let final_cost = rewritten.cost();
        if expected_cost != final_cost {
            println!("*** expected cost {} != final cost {}", expected_cost, final_cost);
        }
        let multiplier = shared.init_cost as f64 / final_cost as f64;
        let multiplier_wrt_orig = very_first_cost as f64 / final_cost as f64;
        let uses = done.usages;
        let use_exprs: Vec<Expr> = done.match_locations.iter().map(|loc| extract(loc.id, &shared.egraph)).collect();
        let use_args: Vec<Vec<Expr>> = done.match_locations.iter().map(|loc|
            (0..done.pattern.arity).map(|ivar|
                loc.arg_shifted_ids[ivar].extract(&shared.egraph)
            ).collect()).collect();
        
        // dreamcoder compatability
        let dc_inv_str: String = dc_inv_str(&inv, past_invs);
        // Rewrite to dreamcoder syntax with all past invention
        // we rewrite "inv1)" and "inv1 " instead of just "inv1" because we dont want to match on "inv10"
        let rewritten_dreamcoder: Vec<String> = rewritten.split_programs().iter().map(|p|{
            let mut res = p.to_string();
            for past_inv in past_invs {
                res = replace_prim_with(&res, &past_inv.inv.name, &past_inv.dc_inv_str);
                // res = res.replace(&format!("{})",past_inv.inv.name), &format!("{})",past_inv.dc_inv_str));
                // res = res.replace(&format!("{} ",past_inv.inv.name), &format!("{} ",past_inv.dc_inv_str));
            }
            res = replace_prim_with(&res, &inv_name, &dc_inv_str);
            // res = res.replace(&format!("{})",inv_name), &format!("{})",dc_inv_str));
            // res = res.replace(&format!("{} ",inv_name), &format!("{} ",dc_inv_str));
            res = res.replace("(lam ","(lambda ");
            res
        }).collect();

        CompressionStepResult { inv, rewritten, rewritten_dreamcoder, done, expected_cost, final_cost, multiplier, multiplier_wrt_orig, uses, use_exprs, use_args, dc_inv_str, initial_cost: shared.init_cost }
    }
    pub fn json(&self) -> serde_json::Value {        
        let use_exprs: Vec<String> = self.use_exprs.iter().map(|expr| expr.to_string()).collect();
        let use_args: Vec<String> = self.use_args.iter().map(|args| format!("{} {}", self.inv.name, args.iter().map(|expr| expr.to_string()).collect::<Vec<String>>().join(" "))).collect();
        let all_uses: Vec<serde_json::Value> = use_exprs.iter().zip(use_args.iter()).sorted().map(|(expr,args)| json!({args: expr})).collect();

        json!({            
            "body": self.inv.body.to_string(),
            "dreamcoder": self.dc_inv_str,
            "arity": self.inv.arity,
            "name": self.inv.name,
            "rewritten": self.rewritten.split_programs().iter().map(|p| p.to_string()).collect::<Vec<String>>(),
            "rewritten_dreamcoder": self.rewritten_dreamcoder,
            "utility": self.done.utility,
            "expected_cost": self.expected_cost,
            "final_cost": self.final_cost,
            "multiplier": self.multiplier,
            "multiplier_wrt_orig": self.multiplier_wrt_orig,
            "num_uses": self.uses,
            "uses": all_uses,
        })
    }
}

impl fmt::Display for CompressionStepResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.expected_cost != self.final_cost {
            write!(f,"[cost mismatch of {}] ", self.expected_cost - self.final_cost)?;
        }
        write!(f, "utility: {} | final_cost: {} | {:.2}x | uses: {} | body: {}",
            self.done.utility, self.final_cost, self.multiplier, self.uses, self.inv)
    }
}

/// calculates the total upper bound on compressive + noncompressive utility
#[inline(never)]
fn utility_upper_bound(
    match_locations: &[MatchLocation],
    body_utility_with_refinement_lower_bound: i32,
    shared: &SharedData,
) -> i32 {
    compressive_utility_upper_bound(match_locations, shared)
        + noncompressive_utility_upper_bound(body_utility_with_refinement_lower_bound, shared)
}

/// This utility is just for any utility terms that we care about that don't directly correspond
/// to changes in size that come from rewriting with an invention
#[inline(never)]
fn noncompressive_utility(
    body_utility_with_refinement: i32,
    cfg: &CompressionStepConfig,
) -> i32 {
    if cfg.no_other_util { return 0; }
    // this is a bit like the structure penalty from dreamcoder except that
    // that penalty uses inlined versions of nested inventions.
    let structure_penalty = - body_utility_with_refinement;
    structure_penalty
}

/// This takes a partial invention and gives an upper bound on the maximum
/// compressive_utility() that any completed offspring of this partial invention could have.
#[inline(never)]
fn compressive_utility_upper_bound(
    match_locations: &[MatchLocation],
    shared: &SharedData,
) -> i32 {
    match_locations.iter().map(|loc|
        compressive_utility_upper_bound_single(loc,shared)
    ).sum::<i32>()
}

/// compressive_utility_upper_bound() for a single location
#[inline]
fn compressive_utility_upper_bound_single(
    loc: &MatchLocation,
    shared: &SharedData,
) -> i32 {
        shared.cost_of_node_all[usize::from(loc.id)] 
        - shared.num_paths_to_node[usize::from(loc.id)] * COST_TERMINAL
}

/// calculates the total upper bound on compressive + noncompressive utility
// #[inline(never)]
// fn utility_upper_bound_with_conflicts(
//     pattern: &Pattern,
//     body_utility_with_refinement_lower_bound: i32,
//     shared: &SharedData,
// ) -> i32 {
//     let utility_of_loc_once: Vec<i32> = pattern.match_locations.iter().map(|node|
//         shared.cost_of_node_once[usize::from(*node)] - COST_TERMINAL).collect();
//     let compressive_utility: i32 = pattern.match_locations.iter()
//         .zip(utility_of_loc_once.iter())
//         .map(|(loc,utility)| utility * shared.num_paths_to_node[usize::from(*loc)])
//         .sum();
//     use_conflicts(pattern, utility_of_loc_once, compressive_utility, shared).util + noncompressive_utility_upper_bound(body_utility_with_refinement_lower_bound, &shared.cfg)
// }


/// This takes a partial invention and gives an upper bound on the maximum
/// other_utility() that any completed offspring of this partial invention could have.
#[inline(never)]
fn noncompressive_utility_upper_bound(
    body_utility_with_refinement_lower_bound: i32,
    shared: &SharedData,
) -> i32 {
    if shared.cfg.no_other_util { return 0; }
    // safe bound: since structure_penalty is negative an upper bound is anything less negative or exact. Since
    // left_utility < body_utility we know that this will be a less negative bound.
    let structure_penalty = - body_utility_with_refinement_lower_bound;
    structure_penalty
}


#[inline(never)]
fn compressive_utility(pattern: &Pattern, match_locations: &[MatchLocation], shared: &SharedData) -> UtilityCalculation {

    // * BASIC CALCULATION
    // Roughly speaking compressive utility is num_usages(invention) * size(invention), however there are a few extra
    // terms we need to take care of too.

    // get a list of (ivar,usages-1) filtering out things that are only used once, this will come in handy for adding multi-use utility later
    let ivar_multiuses: Vec<(usize,i32)> = pattern.arg_zips.iter().map(|labelled|labelled.ivar).counts()
        .iter().filter_map(|(ivar,count)| if *count > 1 { Some((*ivar, (*count-1) as i32)) } else { None }).collect();

    // (parent,child) show up in here if they conflict
    let mut refinement_conflicts: AHashSet<(Id,Id)> = Default::default();
    for r in pattern.refinements.iter().filter_map(|r|r.as_ref()) {
        for ancestor in r.iter() {
            for descendant in r.iter() {
                if ancestor != descendant && is_descendant(*descendant, *ancestor, &shared.egraph) {
                    refinement_conflicts.insert((*ancestor, *descendant));
                }
            }
        }
    }

    // it costs a tiny bit to apply the invention, for example (app (app inv0 x) y) incurs a cost
    // of COST_TERMINAL for the `inv0` primitive and 2 * COST_NONTERMINAL for the two `app`s.
    // Also an extra COST_NONTERMINAL for each argument that is refined (for the lambda).
    let app_penalty = - (COST_TERMINAL + COST_NONTERMINAL * pattern.arity as i32 + COST_NONTERMINAL * pattern.refinements.iter().map(|r|if let Some(refinements) = r {refinements.len() as i32} else {0}).sum::<i32>());


    let utility_of_loc_once: Vec<i32> = match_locations.iter().map(|loc| {
        // println!("calculating util of {}", extract(*loc, &shared.egraph));
        // compressivity of body (no refinement) minus slight penalty from the application
        let base_utility = pattern.body_utility_no_refinement + app_penalty;
        // println!("base {}", base_utility);

        // each use of the refined out arg gives a benefit equal to the size of the arg
        let refinement_utility: i32 = pattern.refinements.iter().enumerate().filter(|(_,r)| r.is_some()).map(|(ivar,r)| {
            unimplemented!(); 0
            // todo old code:
            // let refinements = r.as_ref().unwrap();
            // // grab shifted arg
            // let shifted_arg: Id = shared.arg_of_zid_node[pattern.first_zid_of_ivar[ivar]][loc].shifted_id;
            // if let Some(uses_of_refinement) =  shared.uses_of_shifted_arg_refinement.get(&shifted_arg) {
            //     return refinements.iter().map(|refinement| {
            //         if let Some(uses) = uses_of_refinement.get(&refinement) {
            //             // we subtract COST_TERMINAL because we need to leave behind a $i in place of it in the arg
            //             let mut util = (*uses as i32) * (shared.cost_of_node_once[usize::from(*refinement)] - COST_TERMINAL);
            //             // println!("gained util {} from {}", util, extract(*refinement, &shared.egraph));
            //             for r in refinements.iter().filter(|r| refinement_conflicts.contains(&(*refinement,**r))) {
            //                 // we (an ancestor) conflicted with a descendant so we lose some of that descendants util
            //                 // todo: importantly this doesnt when a grandparent negates both a parent and a child... 
            //                 // todo that would be necessary for 3+ refinements and would be closer to our full conflict resolution setup
            //                 assert!(refinements.len() < 3);
            //                 util -=  (*uses as i32) * (shared.cost_of_node_once[usize::from(*r)] - COST_TERMINAL);
            //             }
            //             util
            //         } else { 0 }
            //     }).sum::<i32>()
            // }
            // // if uses_of_shifted_arg_refinement lacks this shifted_arg then it must not have any refinements so we must not be getting any refinement gain here
            // // likewise if the inner hashmap uses_of_shifted_arg_refinement[shifted_arg] lacks this refinement then we wont get any benefit
            // 0 
        }).sum();

        // println!("refinement {}", refinement_utility);


        // the bad refinement override: if there are any free ivars in the arg at this location (ignoring the refinement itself if there
        // is one) then we can't apply this invention here so *total* util should be 0
        for ivar in 0..pattern.arity {
            let shifted_arg = &loc.arg_shifted_ids[ivar];
            if has_free_ivars(shifted_arg, &pattern.refinements[ivar], &shared.egraph) {
                return 0; // set whole util to 0 for this loc, causing an autoreject
            }
        }

        // for each extra usage of an argument, we gain the cost of that argument as
        // extra utility. Note we use `first_zid_of_ivar` since it doesn't matter which
        // of the zids we use as long as it corresponds to the right ivar
        let multiuse_utility = ivar_multiuses.iter().map(|(ivar,count)|
            count * shared.cost_of_node_once[usize::from(loc.arg_shifted_ids[*ivar].downshifted_id)] // safe: cost of shifted is same as cost of downshifted
        ).sum::<i32>();
        // println!("multiuse {}", multiuse_utility);

        // multiply all this utility by the number of times this node shows up
        base_utility + multiuse_utility + refinement_utility
        }).collect();


    let compressive_utility: i32 = match_locations.iter()
        .zip(utility_of_loc_once.iter())
        .map(|(loc,utility)| utility * shared.num_paths_to_node[usize::from(loc.id)])
        .sum();


    // assertion to make sure pattern.match_locations is sorted (for binary searching + bottom up iterating)
    // {
    //     let mut largest_seen = -1;
    //     assert!(pattern.match_locations.iter().all(|x| {
    //         let res = largest_seen < usize::from(*x) as i32;
    //         largest_seen = usize::from(*x) as i32;
    //         res
    //         }));
    // }

        // * ACCOUNTING FOR USE CONFLICTS:


    use_conflicts(pattern, match_locations, utility_of_loc_once, compressive_utility, shared)
}

#[inline(never)]
fn use_conflicts(pattern: &Pattern, match_locations: &[MatchLocation], utility_of_loc_once: Vec<i32>, compressive_utility: i32, shared: &SharedData) -> UtilityCalculation {

    // zips and ivars
    // note holes will be empty for finished patterns
    let zips: Vec<(&Zip,Option<usize>)> = pattern.arg_zips.iter()
        .map(|labelled_zip| (&labelled_zip.zip,Some(labelled_zip.ivar)))
        .chain(pattern.hole_zips.iter().map(|zip| (zip, None)))
        .collect();

    // the idea here is we want the fast-path to be the case where no conflicts happen. If no conflicts happen, there should be
    // zero heap allocations in this whole section! Since empty vecs and hashmaps dont cause allocations yet.
    let mut corrected_utils: AHashMap<Id,CorrectedUtil> = Default::default();
    let mut global_correction = 0; // this is going to get added to the compressive_utility at the end to correct for use-conflicts

    // bottom up traversal since we assume match_locations is sorted
    for (loc_idx,loc) in match_locations.iter().enumerate() {
        // get all the nodes this could conflict with (by idx within `locs` not by id)
        let conflict_idxs: AHashSet<(Id,usize)> = get_conflicts(&zips, loc.id, shared, pattern, match_locations);

        // now we basically record how much we would affect global utility by if we accept vs reject vs choose the best of those options.
        // and recording this will let us change our mind later if we decide to force-reject something

        // if we reject using the invention at this node, we just lose its utility
        let reject = - utility_of_loc_once[loc_idx];

        // Rare case: when utility_of_loc_once is <=0, then reject is >=0 and of course we should do it
        // (it benefits us or rather brings us back to 0, and leaves maximal flexibility for other things to be accepted/rejected).
        // and theres nothing else we need to account for here.
        if reject >= 0 {
            global_correction += reject * shared.num_paths_to_node[usize::from(loc.id)];
            corrected_utils.insert(loc.id, CorrectedUtil {
                accept: false, // we rejected
                best_util_correction: reject, // we rejected
                util_change_to_reject: 0 // we rejected so no change to reject
            });
            continue
        }

        // common case: no conflicts
        // (this has to come AFTER the possible forced rejection)
        if conflict_idxs.is_empty() { continue; }
        
        // if we accept using the invention at this node everywhere, we lose the util of the difference of the best choice of each descendant vs the reject choice
        // so for example if all the conflicts had chosen to Reject anyways then this would be 0 (optimal)
        // but if some chose to Accept then our Accept correction will include the difference caused by forcing them to reject
        // This is easiest to understand if you think of reject as "the effect on global util of rejecting at a single location"
        // and likewise for accept and best.
        let accept = conflict_idxs.iter()
            .map(|(id,idx)|
                corrected_utils.get(id).map(|x|x.util_change_to_reject)
                // if it's not in corrected_utils, it must have had no conflicts so we must be switching from accept to reject with no other side effects
                // so we do (reject - accept) = (- util(idx) - 0) = - util(idx)
                // where accept was 0 since it caused no conflicts
                .unwrap_or_else(|| - utility_of_loc_once[*idx]) 
            ).sum();

        // lets accept the less negative of the options
        let best_util_correction = std::cmp::max(reject,accept);

        // update global correction with this applied to all our nodes (note that the same choice makes sense for all nodes
        // from the point of view of this being the top of the tree - it's our parents job to use change_to_reject if they
        // want to reject only certain ones of us)
        global_correction += best_util_correction * shared.num_paths_to_node[usize::from(loc.id)];

        let util_change_to_reject = reject - best_util_correction;

        corrected_utils.insert(loc.id, CorrectedUtil {
            accept: best_util_correction == accept,
            best_util_correction,
            util_change_to_reject
        });

        // Involved example:
        // A -> B -> C  (ie A conflicts with B conflicts with C; and A is the parent)
        // and also A -> C
        // 
        // First we calculate C.accept C.reject
        // B.reject as - util(B)
        // B.accept as (C.reject - C.best)
        // A.reject as - util(A)
        // A.accept as (B.reject - B.best) + (C.reject - C.best)
        // did we double count C in here since it was a child of both B and A (I mean literally a child of both not just when struct hashed)
        // if B.best was B.reject, then it would involve allowing C.best to happen so that's good that we have the (C.reject - C.best) term
        //
        // if B.best = B.reject:
        // A.accept = (B.reject - B.best) + (C.reject - C.best)
        //          = (C.reject - C.best) = force C to reject
        // which is good because since B was `reject` then yes A needs to include the C rejection term
        //
        // if B.best = B.accept:
        // A.accept = (B.reject - B.best) + (C.reject - C.best)
        //          = (B.reject - B.accept) + (C.reject - C.best)
        //          = (B.reject - (C.reject - C.best)) + (C.reject - C.best)
        //          = B.reject = - util(B)
        // which is good because we've already modified the global util to incorporate C rejection when we decided that B.best 
        // was B.accept, so it's good that the C terms cancel out here. You can think of what happened like this: we force-reject C
        // which creates a (C.reject - C.best) term, but then we force reject B and since B.best was B.accept which involved C rejection,
        // we get another (C.reject - C.best) term that cancels out the first
        //
        // if B.best was B.accept, then (B.reject - B.best) = (B.reject - B.accept) = (B.reject - (C.reject - C.best))

    }

    UtilityCalculation { util: (compressive_utility + global_correction), corrected_utils}
}

#[inline(never)]
fn get_conflicts(zips: &Vec<(&Zip,Option<usize>)>, loc: Id, shared: &SharedData, pattern: &Pattern, match_locations: &[MatchLocation]) -> AHashSet<(Id, usize)> {
    let mut conflict_idxs = AHashSet::new();
    for (zip,ivar) in zips.iter().filter(|(zip,_)| !zip.is_empty()) {
        let mut id = loc;
        // for all except the last node in the zipper, push the childs location on as a potential conflict
        for znode in zip[..zip.len()-1].iter() {
            // step one deeper
            id = match (znode, &shared.node_of_id[usize::from(id)]) {
                (ZNode::Body, Lambda::Lam([b])) => *b,
                (ZNode::Func, Lambda::App([f,_])) => *f,
                (ZNode::Arg, Lambda::App([_,x])) => *x,
                _ => unreachable!("{:?} {:?}", znode, &shared.node_of_id[usize::from(id)])
            };
            // if its also a location, push it to the conflicts list (do NOT dedup)
            if let Ok(idx) = match_locations.binary_search_by_key(&id, |m| m.id) {
                conflict_idxs.insert((id,idx));
            }
        }
        // if this is a refinement, push every descendant of the unshifted argument including it itself as a potential conflict
        if let Some(ivar) = ivar {
            if pattern.refinements[*ivar].is_some() {
                #[inline(never)]
                fn helper(id: Id, shared: &SharedData, conflict_idxs: &mut AHashSet<(Id,usize)>, pattern: &Pattern, match_locations: &[MatchLocation]) {
                    if let Ok(idx) = match_locations.binary_search_by_key(&id, |m| m.id) {
                        conflict_idxs.insert((id,idx));
                    }
                    match &shared.node_of_id[usize::from(id)] {
                        Lambda::Lam([b]) => {helper(*b, shared, conflict_idxs, pattern,  match_locations);},
                        Lambda::App([f,x]) => {
                            helper(*f, shared, conflict_idxs, pattern, match_locations);
                            helper(*x, shared, conflict_idxs, pattern, match_locations);
                        }
                        Lambda::Prim(_) | Lambda::Var(_) | Lambda::IVar(_) => {},
                        _ => unreachable!()
                    }
                }
                // we want to conflict with everything within the arg (for simplicity) so lets recurse on the unshifted arg
                // but ack thats slightly annoying to get here so lets leave it as a todo for later
                unimplemented!()
                // todo old code:
                // let unshifted_arg: Id = shared.arg_of_zid_node[pattern.first_zid_of_ivar[*ivar]][loc].unshifted_id;
                // helper(unshifted_arg, shared, &mut conflict_idxs, pattern);
            }
        }
    }
    // conflict_idxs.range(..).into_iter().collect()
    conflict_idxs
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtilityCalculation {
    pub util: i32,
    pub corrected_utils: AHashMap<Id,CorrectedUtil>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectedUtil {
    pub accept: bool, // whether it's the best choice to accept applying the invention at this node when there are no other parent nodes above us (ignoring context, would this be the right choice?)
    pub best_util_correction: i32, // the change in utility that this choice would cause. Always <= 0
    pub util_change_to_reject: i32, // if accept=false this is 0 otherwise it's the difference in utility between accept and reject. Always <= 0.
}



/// Multistep compression. See `compression_step` if you'd just like to do a single step of compression.
pub fn compression(
    programs_expr: &Expr,
    iterations: usize,
    cfg: &CompressionStepConfig,
    tasks: &Vec<String>,
    num_prior_inventions: usize,
) -> Vec<CompressionStepResult> {

    let mut rewritten: Expr = programs_expr.clone();
    let mut step_results: Vec<CompressionStepResult> = Default::default();

    let tstart = std::time::Instant::now();

    for i in 0..iterations {
        println!("{}",format!("\n=======Iteration {}=======",i).blue().bold());
        let inv_name = format!("fn_{}", num_prior_inventions + step_results.len());

        // call actual compression
        let res: Vec<CompressionStepResult> = compression_step(
            &rewritten,
            &inv_name,
            &cfg,
            &step_results,
            tasks);

        if !res.is_empty() {
            // rewrite with the invention
            let res: CompressionStepResult = res[0].clone();
            rewritten = res.rewritten.clone();
            println!("Chose Invention {}: {}", res.inv.name, res);
            step_results.push(res);
        } else {
            println!("No inventions found at iteration {}",i);
            break;
        }
    }

    if cfg.dreamcoder_drop_last {
        println!("{}",format!("{}","[--dreamcoder-drop-last] dropping final invention".yellow().bold()));
        step_results.pop();
    }

    println!("{}","\n=======Compression Summary=======".blue().bold());
    println!("Found {} inventions", step_results.len());
    println!("Cost Improvement: ({:.2}x better) {} -> {}", compression_factor(programs_expr,&rewritten), programs_expr.cost(), rewritten.cost());
    for i in 0..step_results.len() {
        let res = &step_results[i];
        println!("{} ({:.2}x wrt orig): {}" ,res.inv.name.clone().blue(), compression_factor(programs_expr, &res.rewritten), res);
    }
    println!("Time: {}ms", tstart.elapsed().as_millis());
    if cfg.follow_track && !(
        cfg.no_opt_free_vars
        && cfg.no_opt_single_task
        && cfg.no_opt_upper_bound
        && cfg.no_opt_force_multiuse
        && cfg.no_opt_useless_abstract
        && cfg.no_opt_arity_zero)
    {
        println!("{} you often want to run --follow-track with --no-opt otherwise your target may get pruned", "[WARNING]".yellow());
    }
    step_results
}

/// Takes a set of programs as an Expr with Programs as its root, and does one full step of compresison.
/// Returns the top Inventions and the Expr rewritten under that invention along with other useful info in CompressionStepResult
/// The number of inventions returned is based on cfg.inv_candidates
pub fn compression_step(
    programs_expr: &Expr,
    new_inv_name: &str, // name of the new invention, like "inv4"
    cfg: &CompressionStepConfig,
    past_invs: &Vec<CompressionStepResult>, // past inventions we've found
    tasks: &Vec<String>,
) -> Vec<CompressionStepResult> {

    let tstart_total = std::time::Instant::now();
    let tstart_prep = std::time::Instant::now();
    let mut tstart = std::time::Instant::now();

    // build the egraph. We'll just be using this as a structural hasher we don't use rewrites at all. All eclasses will always only have one node.
    let mut egraph: EGraph = Default::default();
    let programs_node = egraph.add_expr(programs_expr.into());
    egraph.rebuild();
    let init_cost = egraph[programs_node].data.inventionless_cost;

    println!("set up egraph: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    let roots: Vec<Id> = egraph[programs_node].nodes[0].children().iter().cloned().collect();

    // all nodes in child-first order except for the Programs node
    let mut treenodes: Vec<Id> = topological_ordering(programs_node,&egraph);
    assert!(treenodes.iter().enumerate().all(|(i,node)| i == usize::from(*node)));
    // assert_eq!(treenodes.iter().map(|n| usize::from(*n)).collect::<Vec<_>>(), (0..treenodes.len()).collect::<Vec<_>>());
    let node_of_id: Vec<Lambda> = treenodes.iter().map(|node| egraph[*node].nodes[0].clone()).collect();
    treenodes.retain(|id| *id != programs_node);

    println!("got roots, treenodes, and cloned egraph contents: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    // populate num_paths_to_node so we know how many different parts of the programs tree
    // a node participates in (ie multiple uses within a single program or among programs)
    let num_paths_to_node: Vec<i32> = num_paths_to_node(&roots, &treenodes, &egraph);

    println!("num_paths_to_node(): {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    let tasks_of_node: Vec<AHashSet<usize>> = associate_tasks(programs_node, &egraph, &treenodes, tasks);

    println!("associate_tasks(): {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();


    // todo move into a function
    let mut shifted_of_id: Vec<Shifted> = vec![];
    for id in treenodes.iter() {
        if egraph[*id].data.free_vars.is_empty() || *egraph[*id].data.free_vars.iter().min().unwrap() == 0 {
            shifted_of_id.push(Shifted::new(*id,0));
        } else {
            let shift_by = *egraph[*id].data.free_vars.iter().min().unwrap();
            let new_id = shift(*id, -shift_by, &mut egraph, &mut None).unwrap();
            shifted_of_id.push(Shifted::new(new_id,shift_by));
        }
    }

    egraph.rebuild();

    // extended treenodes: literally just take everything in the egraph
    let extended_treenodes: Vec<Id> = (0..egraph.number_of_classes()).map(|id| Id::from(id)).collect();
    // let extended_treenodes: Vec<Id> = (0..usize::from(shifted_of_id.iter().map(|s| s.id).max().unwrap())).map(|u| Id::from(u)).collect();

    println!("shifted_of_id struct: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    // cost of a single usage of a node (same as inventionless_cost)
    let cost_of_node_once: Vec<i32> = extended_treenodes.iter().map(|node| egraph[*node].data.inventionless_cost).collect();
    // cost of a single usage times number of paths to node - no extended_treenodes since num_paths doesnt make sense for things that arent in the normal set of treenodes
    let cost_of_node_all: Vec<i32> = treenodes.iter().map(|node| cost_of_node_once[usize::from(*node)] * num_paths_to_node[usize::from(*node)]).collect();

    let free_vars_of_node: Vec<AHashSet<i32>> = extended_treenodes.iter().map(|node| egraph[*node].data.free_vars.clone()).collect();

    println!("cost_of_node and free_vars_of_node structs: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();


    // let (zid_of_zip,
    //     zip_of_zid,
    //     arg_of_zid_node,
    //     zids_of_node,
    //     extensions_of_zid,
    //     uses_of_shifted_arg_refinement) = get_zippers(&treenodes, &cost_of_node_once, cfg.no_cache, &mut egraph, cfg);
    
    // println!("get_zippers(): {:?}ms", tstart.elapsed().as_millis());
    // tstart = std::time::Instant::now();
    
    // println!("{} zips", zip_of_zid.len());
    // println!("arg_of_zid_node size: {}", arg_of_zid_node.len());

    // set up tracking if any
    let tracking: Option<Tracking> = cfg.track.as_ref().map(|s|{
        let expr: Expr = s.parse().unwrap();
        let zips_of_ivar = zips_of_ivar_of_expr(&expr);
        let refined = cfg.track_refined.as_ref().map(|s| s.parse().unwrap());
        Tracking { expr, zips_of_ivar, refined }
    });

    println!("Tracking setup: {:?}ms", tstart.elapsed().as_millis());

    let mut stats: Stats = Default::default();

    tstart = std::time::Instant::now();

    // define all the important data structures for compression
    let mut donelist: Vec<FinishedPattern> = Default::default(); // completed inventions will go here    

    // arity 0 inventions
    if !cfg.no_opt_arity_zero {
        for node in treenodes.iter() {

            // check for free vars: inventions with free vars in the body are not well-defined functions
            // and should thus be discarded
            if !cfg.no_opt_free_vars && !egraph[*node].data.free_vars.is_empty() {
                if !cfg.no_stats { stats.free_vars_fired += 1; };
                continue;
            }

            // check whether this invention is useful in > 1 task
            if !cfg.no_opt_single_task && tasks_of_node[usize::from(*node)].len() < 2 {
                if !cfg.no_stats { stats.single_task_fired += 1; };
                continue;
            }
            // Note that "single use" pruning is intentionally not done here,
            // since any invention specific to a node will by definition only
            // be useful at that node

            let match_locations = vec![MatchLocation::new(*node)];
            let body_utility_no_refinement = cost_of_node_once[usize::from(*node)];
            let refinement_body_utility = 0;
            // compressive_utility for arity-0 is cost_of_node_all[node] minus the penalty of using the new prim
            let compressive_utility = cost_of_node_all[usize::from(*node)] - num_paths_to_node[usize::from(*node)] * COST_TERMINAL;
            let utility = compressive_utility + noncompressive_utility(body_utility_no_refinement + refinement_body_utility, cfg);
            if utility <= 0 { continue; }

            let pattern = Pattern {
                // holes: vec![],
                // arg_choices: vec![],
                // first_zid_of_ivar: vec![],
                refinements: vec![],
                // match_locations:Some(match_locations),
                utility_upper_bound: utility,
                body_utility_no_refinement,
                refinement_body_utility,
                tracked: false,
                hole_zips: vec![],
                // hole_unshifted_ids: vec![],
                arg_zips: vec![],
                // arg_shifted_ids: vec![],
                arity: 0,
                any_loc_id: *node,
            };
            let finished_pattern = FinishedPattern {
                pattern,
                match_locations,
                utility,
                compressive_utility,
                util_calc: UtilityCalculation { util: compressive_utility, corrected_utils: Default::default()},
                arity: 0,
                usages: num_paths_to_node[usize::from(*node)]
            };
            donelist.push(finished_pattern);
        }
    }

    println!("arity 0: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    println!("got {} arity zero inventions", donelist.len());

    let crit = CriticalMultithreadData::new(donelist, &treenodes, &cost_of_node_all, &num_paths_to_node, &node_of_id, &cfg);
    let shared = Arc::new(SharedData {
        crit: Mutex::new(crit),
        max_heapkey: Mutex::new(cfg.heap_choice.init()),
        // arg_of_zid_node,
        treenodes: treenodes.clone(),
        node_of_id: node_of_id,
        programs_node,
        roots,
        // zids_of_node,
        // zip_of_zid,
        // zid_of_zip,
        // extensions_of_zid,
        // uses_of_shifted_arg_refinement,
        egraph,
        num_paths_to_node,
        tasks_of_node,
        cost_of_node_once,
        cost_of_node_all,
        free_vars_of_node,
        shifted_of_id,
        init_cost,
        stats: Mutex::new(stats),
        cfg: cfg.clone(),
        tracking,
    });

    // { // scoping to ensure lock drops, not sure if this is needed
    //     shared.crit.lock().deref_mut().worklist.push(HeapItem::new(Pattern::single_hole(&*shared), &*shared));
    // }

    println!("built SharedData: {:?}ms", tstart.elapsed().as_millis());
    tstart = std::time::Instant::now();

    if cfg.verbose_best {
        let mut crit = shared.crit.lock();
        if !crit.deref_mut().donelist.is_empty() {
            let best_util = crit.deref_mut().donelist.first().unwrap().utility;
            let best_expr: String = crit.deref_mut().donelist.first().unwrap().info(&shared);
            println!("{} @ step=0 util={} for {}", "[new best utility]".blue(), best_util, best_expr);
        }
    }

    println!("TOTAL PREP: {:?}ms", tstart_prep.elapsed().as_millis());

    println!("running pattern search...");

    // *****************
    // * STITCH SEARCH *
    // *****************
    // (this is finding all the higher-arity multi-use inventions through stitching)
    if cfg.threads == 1 {
        // Single threaded
        stitch_search(Arc::clone(&shared));
    } else {
        // Multithreaded
        let mut handles = vec![];
        for _ in 0..cfg.threads {
            // clone the Arcs to have copies for this thread
            let shared = Arc::clone(&shared);
            
            // launch thread to just call stitch_search()
            handles.push(thread::spawn(move || {
                stitch_search(shared);
            }));
        }
        // wait for all threads to finish (when all have empty worklists)
        for handle in handles {
            handle.join().unwrap();
        }
    }

    println!("TOTAL SEARCH: {:?}ms", tstart.elapsed().as_millis());
    println!("TOTAL PREP + SEARCH: {:?}ms", tstart_total.elapsed().as_millis());


    tstart = std::time::Instant::now();

    // at this point we hold the only reference so we can get rid of the Arc
    let mut shared: SharedData = Arc::try_unwrap(shared).unwrap();

    // one last .update()
    shared.crit.lock().deref_mut().update(&cfg);

    println!("{:?}", shared.stats.lock().deref_mut());
    assert!(shared.crit.lock().deref_mut().worklist.is_empty());

    let donelist: Vec<FinishedPattern> = shared.crit.lock().deref_mut().donelist.clone();

    if cfg.dreamcoder_comparison {
        println!("Timing point 1 (from the start of compression_step to final donelist): {:?}ms", tstart_total.elapsed().as_millis());
        println!("Timing Comparison Point A (search) (millis): {}", tstart_total.elapsed().as_millis());
        let tstart_rewrite = std::time::Instant::now();
        rewrite_fast(&donelist[0], &shared, new_inv_name);
        println!("Timing point 2 (rewriting the candidate): {:?}ms", tstart_rewrite.elapsed().as_millis());
        println!("Timing Comparison Point B (search+rewrite) (millis): {}", tstart_total.elapsed().as_millis());
    }

    let mut results: Vec<CompressionStepResult> = vec![];

    // construct CompressionStepResults and print some info about them)
    println!("Cost before: {}", shared.init_cost);
    for (i,done) in donelist.iter().enumerate() {
        let res = CompressionStepResult::new(done.clone(), new_inv_name, &mut shared, past_invs);

        println!("{}: {}", i, res);
        if cfg.show_rewritten {
            println!("rewritten:\n{}", res.rewritten.split_programs().iter().map(|p|p.to_string()).collect::<Vec<_>>().join("\n"));
        }
        results.push(res);
    }
    println!("post stuff: {:?}ms", tstart.elapsed().as_millis());

    results
}