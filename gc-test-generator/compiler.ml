(* String-only GC test generator. Standard library + Str; no compiler/runtime dependency. *)
exception Error of int * string
let fail line fmt = Printf.ksprintf (fun s -> raise (Error (line, s))) fmt
let matches re s = Str.string_match (Str.regexp ("^" ^ re ^ "$")) s 0
let temp s = matches "t[0-9]+" s
let reserved = ["heap"; "expect"; "success"; "oom"; "func"; "end"; "alloc"; "call"; "null"]
let ident line s =
  if not (matches "[A-Za-z_][A-Za-z0-9_]*" s) || List.mem s reserved then
    fail line "invalid or reserved identifier %S" s;
  s
let number line s =
  if not (matches "[0-9]+" s) then fail line "expected a nonnegative decimal integer, got %S" s;
  match int_of_string_opt s with
  | Some n when n <= 2147483647 -> n
  | _ -> fail line "integer exceeds 2147483647"

type rhs = Ref of string | Null | Empty
type stmt = Alloc of int * string * char | Assign of string * rhs | Call of string
type func = { name : string; arity : int; line : int; body : (int * stmt) list }
type program = { heap : int; oom : bool; funcs : func list }
let read_lines path =
  let ch = open_in path in
  Fun.protect ~finally:(fun () -> close_in ch) (fun () ->
    let rec loop n acc = match input_line ch with
    | s ->
      let s = match String.index_opt s '#' with Some i -> String.sub s 0 i | None -> s in
      let s = String.trim s in
      loop (n+1) (if s = "" then acc else (n,s)::acc)
    | exception End_of_file -> List.rev acc in loop 1 [])
let words s = Str.split (Str.regexp "[ \t\r]+") s
let parse path =
  let rest = ref (read_lines path) in
  let take () = match !rest with
    | x::xs -> rest := xs; x
    | [] -> fail 0 "unexpected end of input" in
  let line,s = take () in
  let heap = match words s with
    | ["heap"; n] -> let n=number line n in if n=0 then fail line "heap must be positive"; n
    | _ -> fail line "expected heap <bytes>" in
  let line,s = take () in
  let oom = match words s with
    | ["expect"; "success"] -> false | ["expect"; "oom"] -> true
    | _ -> fail line "expected expect success or expect oom" in
  let site = ref 0 in
  let statement line s = match words s with
    | ["alloc"; n; dst] ->
      let c = Char.chr (Char.code 'a' + !site mod 26) in incr site;
      Alloc (number line n, ident line dst, c)
    | ["call"; name] -> Call (ident line name)
    | _ -> (match String.split_on_char '=' s with
      | [dst; value] ->
        let dst = ident line (String.trim dst) in
        let value = String.trim value in
        let rhs = match value with "null" -> Null | "\"\"" -> Empty | _ -> Ref (ident line value) in
        Assign (dst,rhs)
      | _ -> fail line "expected alloc N name, name = value, or call function") in
  let rec functions acc = match !rest with
    | [] -> List.rev acc
    | _ ->
      let line,s = take () in
      let name,arity = match words s with
        | ["func"; name; k] -> ident line name, number line k
        | _ -> fail line "expected func <name> <arity>" in
      if arity > 256 then fail line "arity exceeds generator resource limit 256";
      if List.exists (fun (f : func) -> f.name = name) acc then fail line "duplicate function %s" name;
      let rec body acc =
        let l,s = take () in if s="end" then List.rev acc
        else body ((l,statement l s)::acc) in
      let f={name;arity;line;body=body []} in functions (f::acc) in
  let funcs=functions [] in
  (match List.find_opt (fun (f : func) -> f.name="main") funcs with
   | Some f when f.arity=0 -> () | _ -> fail 0 "exactly one func main 0 is required");
  {heap;oom;funcs}

let validate p =
  let lookup l n = match List.find_opt (fun (f : func) -> f.name=n) p.funcs with
    | Some f -> f | None -> fail l "unknown function %s" n in
  List.iter (fun (f : func) ->
    let defined = Hashtbl.create 16 in
    for i=0 to f.arity-1 do Hashtbl.add defined ("arg"^string_of_int i) () done;
    let read l n = if not (Hashtbl.mem defined n) then fail l "read before assignment: %s" n in
    List.iter (fun (l,s) -> match s with
      | Call n -> ignore (lookup l n)
      | Alloc (_,dst,_) | Assign (dst,_) ->
        (match s with Assign (_,Ref n) -> read l n | _ -> ());
        if matches "arg[0-9]+" dst && not (List.exists (fun i -> dst="arg"^string_of_int i) (List.init f.arity Fun.id)) then
          fail l "undeclared reserved parameter %s" dst;
        if temp dst && Hashtbl.mem defined dst then fail l "temporary %s is assigned more than once" dst;
        (match s with Assign (_,Null) when temp dst -> fail l "temporary %s cannot be null" dst | _ -> ());
        Hashtbl.replace defined dst ()) f.body) p.funcs;
  let done_ = Hashtbl.create 16 in
  let rec visit path f =
    if List.mem f.name path then fail f.line "recursive call graph involving %s" f.name;
    if not (Hashtbl.mem done_ f.name) then begin
      List.iter (fun (l,s) -> match s with Call n -> visit (f.name::path) (lookup l n) | _ -> ()) f.body;
      Hashtbl.add done_ f.name ()
    end in
  List.iter (visit []) p.funcs;
  (* Without an invocation there is no argument context to validate. Reject unused
     functions rather than silently accepting potentially invalid dead definitions. *)
  let reached=Hashtbl.create 16 in
  let rec reach f = if not (Hashtbl.mem reached f.name) then begin
    Hashtbl.add reached f.name ();
    List.iter (fun (l,s) -> match s with Call n -> reach (lookup l n) | _ -> ()) f.body end in
  reach (lookup 0 "main");
  List.iter (fun (f : func) -> if not (Hashtbl.mem reached f.name) then fail f.line "unreachable function %s" f.name) p.funcs

type value = { id : int; len : int; ch : char }
type binding = { name : string; slot : string; mutable value : value option }
type generated = { method_name : string; code : string; output : string; requests : int64 }
let quote c n = "\"" ^ String.make n c ^ "\""
let generate p threshold =
  let cache = Hashtbl.create 32 and methods = ref [] and count = ref 0 in
  let limit = 32 * 1024 * 1024 in
  let add b s = if Buffer.length b + String.length s > limit then fail 0 "generated method/output exceeds 32 MiB resource limit"; Buffer.add_string b s in
  let literal l v =
    if v.len > threshold then fail l "temporary literal has %d characters; limit is %d (raise --literal-limit or use an ordinary name)" v.len threshold;
    (* Separate safety bound for accidental multi-GiB source generation. *)
    if v.len > limit then fail l "literal exceeds 32 MiB generator resource limit";
    quote v.ch v.len in
  let normalize args =
    let ids = List.sort_uniq compare (List.map (fun v -> v.id) args) in
    let rec rank n id = function x::_ when x=id -> n | _::xs -> rank (n+1) id xs | [] -> assert false in
    List.map (fun v -> {v with id=rank 0 v.id ids}) args in
  let rec compile (f : func) incoming =
    let args=normalize incoming in
    let key=f.name, List.map (fun v -> v.id,v.len,v.ch) args in
    match Hashtbl.find_opt cache key with Some g -> g | None ->
    if !count >= 4096 then fail f.line "more than 4096 specialized methods";
    let method_name=if f.name="main" then "main" else "m" ^ string_of_int !count in
    incr count;
    let body=Buffer.create 1024 and expected=Buffer.create 1024 and requests=ref 0L in
    let env=ref (List.mapi (fun i v -> {name="arg"^string_of_int i;slot="p"^string_of_int i;value=Some v}) args) in
    let next=ref (List.length args) in
    let fresh n c = let v={id= !next;len=n;ch=c} in incr next; v in
    let get l n = match List.find_opt (fun (b : binding) -> b.name=n) !env with Some b -> b | None -> fail l "read before assignment: %s" n in
    let expr l b = match b.value with
      | None -> "gcEmpty" | Some v when temp b.name -> literal l v | Some _ -> b.slot in
    let emit s = add body ("        "^s^"\n") in
    let set l dst v expression =
      if temp dst then (match v with None -> fail l "temporary %s cannot be null" dst | Some v -> ignore(literal l v));
      let b=match List.find_opt (fun (b : binding)->b.name=dst) !env with
        | Some b -> if temp dst then fail l "temporary %s assigned twice" dst; b
        | None -> let b={name=dst;slot="v"^string_of_int(List.length !env);value=None} in env:= !env@[b]; b in
      if not(temp dst) then emit (b.slot ^ " = " ^ expression ^ ";");
      b.value <- v in
    let observe ?only () =
      List.iter (fun (b : binding) -> if not(temp b.name) &&
        (only=None || only=Some b.name) then match b.value with
        | None -> ()
        | Some v when p.oom ->
          let stop="return 1;" in
          let yes,no=if v.len=0 then ";",stop else stop,";" in
          emit ("if (("^b.slot^" = gcEmpty)) { "^yes^" } else { "^no^" }")
        | Some v ->
          let rhs = if v.len=0 then "gcEmpty" else Printf.sprintf "(\"%c\" * %d)" v.ch v.len in
          emit ("out.print_bool(("^b.slot^" = "^rhs^")); out.println();");
          add expected "true\n") !env in
    observe ();
    List.iter (fun (l,s) ->
      (match s with
      | Alloc(n,dst,c) ->
        let v=fresh n c in
        let rhs=if temp dst then literal l v else if n=0 then "gcEmpty" else Printf.sprintf "(\"%c\" * %d)" c n in
        if not(temp dst) then requests:=Int64.add !requests (Int64.of_int n);
        set l dst (Some v) rhs
      | Assign(dst,rhs) ->
        let v,e = match rhs with
          | Null -> None,"gcEmpty" | Empty -> Some(fresh 0 'x'),"gcEmpty"
          | Ref n -> let b=get l n in b.value,expr l b in
        set l dst v e
      | Call name ->
        observe ();
        let callee=List.find (fun (f : func) -> f.name=name) p.funcs in
        let seen=Hashtbl.create 16 in
        let candidates=List.filter_map (fun (b : binding) -> match b.value with
          | Some v when not(Hashtbl.mem seen v.id) -> Hashtbl.add seen v.id (); Some(b,v)
          | _ -> None) !env |> List.sort (fun (_,a) (_,b) -> compare b.id a.id) in
        if List.length candidates < callee.arity then fail l "%s requires %d distinct live values; only %d available" name callee.arity (List.length candidates);
        let chosen=List.filteri (fun i _ -> i<callee.arity) candidates in
        let g=compile callee (List.map snd chosen) in
        emit ((if p.oom then "gcStatus = " else "")^"this."^g.method_name^"("^String.concat ", " (List.map (fun (b,_) -> expr l b) chosen)^");");
        if p.oom then emit "if ((gcStatus = 0)) { ; } else { return 1; }";
        add expected g.output;
        requests:=Int64.add !requests g.requests);
      (match s with
       | Alloc(_,dst,_) | Assign(dst,_) when not p.oom -> observe ~only:dst ()
       | _ -> observe ())) f.body;
    observe ();
    if f.name="main" then emit (if p.oom then "return 2;" else "return 0;")
    else if p.oom then emit "return 0;"
    else if Buffer.length body=0 then emit ";";
    let locals=List.filter (fun (b : binding) -> not(temp b.name) && not(matches "arg[0-9]+" b.name)) !env in
    let params=List.mapi(fun i _ -> "String p"^string_of_int i) args |> String.concat ", " in
    let code="    "^(if f.name="main" || p.oom then "int" else "void")^" "^method_name^"("^params^") {\n"^
      (if p.oom then "        int gcStatus;\n" else "")^
      String.concat "" (List.map(fun (b : binding)->"        String "^b.slot^";\n") locals)^Buffer.contents body^"    }\n" in
    let g={method_name;code;output=Buffer.contents expected;requests= !requests} in
    Hashtbl.add cache key g; methods:=g::!methods; g in
  let main=compile (List.find(fun (f : func)->f.name="main") p.funcs) [] in
  let header=if p.oom then "// runtime abort test case\n// expected exit code: 137\n// expected stderr substring: lo_alloc: out of memory\n" else "// good test case\n// main method return value: 0\n" in
  let code=header^Printf.sprintf "// gc stress heap: %d\n// Generated by gc-test-generator; do not edit.\n// Ordinary requested payload bytes: %Ld (excludes checking/literals).\n// Allocation sites use a..z cyclically in source order.\n// Observation profile: assigned value after each write; all roots at function/call boundaries.\n" p.heap main.requests^
    "class Main ( String gcEmpty; ) [ Main() { ; } ] {\n"^
    String.concat "" (List.map (fun g->g.code) (List.rev !methods))^"}\n" in
  if String.length code > limit then fail 0 "generated LO exceeds 32 MiB limit";
  let output=if not p.oom && main.output="" then "true\n" else main.output in
  let code=if not p.oom && main.output="" then Str.global_replace (Str.regexp_string "return 0;") "out.print_bool(true); out.println(); return 0;" code else code in
  code,output

let write path s = let c=open_out_bin path in Fun.protect ~finally:(fun()->close_out c) (fun()->output_string c s)
let () =
  let src=ref "" and output=ref "" and expected=ref "" and threshold=ref 256 in
  let options=["-o",Arg.Set_string output,"PATH Output .lo (default: beside source)";
    "--expected",Arg.Set_string expected,"PATH Expected stdout (default: output stem.expected.out)";
    "--literal-limit",Arg.String(fun s -> threshold:= if s="unlimited" then max_int else number 0 s),"N|unlimited Maximum t<int> literal length (default 256)"] in
  try
    Arg.parse options (fun s -> if !src<>"" then fail 0 "provide exactly one input";src:=s) "gc-metalang [options] program.lo_gc";
    if !src="" || not(Filename.check_suffix !src ".lo_gc") then fail 0 "input must have extension .lo_gc";
    if !output="" then output:=Filename.chop_suffix !src ".lo_gc"^".lo";
    if not(Filename.check_suffix !output ".lo") then fail 0 "output must have extension .lo";
    if !expected="" then expected:=Filename.chop_suffix !output ".lo"^".expected.out";
    if !expected= !src || !expected= !output then fail 0 "input/output paths must differ";
    let p=parse !src in validate p;
    let code,exp=generate p !threshold in
    (* All semantic validation precedes output. Parent directories must exist. *)
    List.iter (fun path -> if not(Sys.is_directory(Filename.dirname path)) then fail 0 "output parent must be a directory") [!output;!expected];
    write !output code; write !expected exp
  with
  | Error(l,s) -> Printf.eprintf "%s:%d: error: %s\n" !src l s; exit 1
  | Sys_error s -> Printf.eprintf "error: %s\n" s; exit 1
