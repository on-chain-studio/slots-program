use slots_engine::*;
fn machine() -> (MachineConfig, RunRules) {
 let mut c=MachineConfig{stake_lamports:900,reel_count:5,strip_len:3,row_count:3,symbol_count:2,line_count:1,..Default::default()};
 c.strips=[[0,1,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0];5];
 let mut rules=RunRules::default();rules.spans[0]=Span{start:0,count:5};rules.pays[0]=[0,0,10,30,100];
 (c,rules)
}
#[test]
fn horizontal_runs_start_anywhere_and_pay_only_the_maximal_length(){
 let (c,rules)=machine();
 for (stops,start,count,mult) in [([0,0,0,1,1],0,3,10),([1,0,0,0,1],1,3,10),([1,1,0,0,0],2,3,10),([1,0,0,0,0],1,4,30),([0;5],0,5,100)] {
  let (paid,wins)=value_runs(&c,&rules,&stops).unwrap();assert_eq!(paid.lamports,900*mult);assert_eq!(paid.lines,1);assert_eq!((wins[0].start,wins[0].count),(start,count));
 }
 assert_eq!(value_runs(&c,&rules,&[0,0,1,0,0]).unwrap().0.lamports,0);
}
#[test]
fn offset_diagonals_and_crossing_lines_pay_independently(){
 let (mut c,mut rules)=machine();c.line_count=2;c.lines[0].rows=[0,0,1,2,0];c.lines[1].rows=[0,2,1,0,0];
 rules.spans[0]=Span{start:1,count:3};rules.spans[1]=Span{start:1,count:3};
 c.strips[1][2]=0;c.strips[3][2]=0;
 let (w,hits)=value_runs(&c,&rules,&[1,0,2,0,1]).unwrap();assert_eq!(w.lamports,9000);assert_eq!(w.lines,3);
 assert_eq!((hits[0].start,hits[1].start),(1,1));
 rules.spans[0]=Span{start:2,count:4};assert!(value_runs(&c,&rules,&[0;5]).is_err());
}
#[test]
fn legacy_rules_and_duplicate_segments(){
 let (mut c,mut rules)=machine();c.symbols[0].mult=100;
 assert_eq!(value(&c,&[1,0,0,0,1]).unwrap().lamports,0);
 assert_eq!(value(&c,&[0;5]).unwrap().lamports,90000);
 c.line_count=2;rules.spans[1]=Span{start:1,count:3};assert!(rules.check(&c).is_err());
}

#[test]
fn whole_bet_payouts_preserve_lamports_with_nine_lines(){
 let (mut c,mut rules)=machine();c.stake_lamports=100_000_000;c.line_count=9;c.strips=[[0;32];5];
 for i in 0..3 {c.lines[i].rows=[i as u8;5];rules.spans[i]=Span{start:0,count:5};}
 for start in 0..3 {for direction in 0..2 {let i=3+start*2+direction;rules.spans[i]=Span{start:start as u8,count:3};for n in 0..3 {c.lines[i].rows[start+n]=if direction==0 {n as u8}else{2-n as u8};}}}
 rules.pays[0]=[0,0,18,18,18];
 assert_eq!(value_runs(&c,&rules,&[0;5]).unwrap().0.lamports,1_800_000_000);
}
